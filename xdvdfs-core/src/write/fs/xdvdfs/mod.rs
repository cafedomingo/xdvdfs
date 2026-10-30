use std::collections::{BTreeMap, BTreeSet};
use std::io;

use crate::Error;
use crate::blockdev::{BlockDeviceRead, BlockDeviceWrite};
use crate::layout::DirectoryEntryNode;
use crate::read::TableRanges;

use super::FilesystemCopier;
use super::FilesystemHierarchy;
use super::ImagePath;
use super::{FileEntry, FileType};

mod copier;
pub use copier::*;

/// A Filesystem backed by an XDVDFS block device
/// Reads entries and data from a supplied XDVDFS image
pub struct XDVDFSFilesystem<D, W, Copier = DefaultCopier<D, W>>
where
    D: BlockDeviceRead + Sized,
    W: BlockDeviceWrite + ?Sized,
{
    dev: D,
    volume: crate::layout::VolumeDescriptor,
    /// Each entry `read_dir` listed. Lookups try it before `walk_path`, since
    /// other writers' images can list entries their tree search can't reach.
    entries: BTreeMap<ImagePath, DirectoryEntryNode>,
    /// The path that first read each table with entries
    table_paths: TableRanges<ImagePath>,
    copier: Copier,

    w_type: core::marker::PhantomData<W>,
}

impl<D, W, Copier> XDVDFSFilesystem<D, W, Copier>
where
    D: BlockDeviceRead + Sized,
    W: BlockDeviceWrite + ?Sized,
    Copier: RWCopier<D, W>,
{
    pub fn new(mut dev: D) -> crate::Result<XDVDFSFilesystem<D, W, Copier>> {
        let volume = crate::read::read_volume(&mut dev)?;
        Ok(Self {
            dev,
            volume,
            entries: BTreeMap::new(),
            table_paths: TableRanges::default(),
            copier: Copier::default(),
            w_type: core::marker::PhantomData,
        })
    }

    fn lookup(&mut self, path: &ImagePath) -> crate::Result<DirectoryEntryNode> {
        if let Some(dirent) = self.entries.get(path) {
            return Ok(*dirent);
        }

        self.volume
            .root_table
            .walk_path(&mut self.dev, path)
            .map_err(|err| match err {
                Error::DoesNotExist | Error::DirectoryEmpty | Error::IsNotDirectory => {
                    Error::NotFound(path.clone())
                }
                err => err,
            })
    }
}

impl<D, W, Copier> FilesystemHierarchy for XDVDFSFilesystem<D, W, Copier>
where
    D: BlockDeviceRead + Sized,
    W: BlockDeviceWrite + ?Sized,
    Copier: RWCopier<D, W>,
{
    fn read_dir(&mut self, dir: &ImagePath) -> crate::Result<Vec<FileEntry>> {
        let dirtab = if dir.is_root() {
            self.volume.root_table
        } else {
            self.lookup(dir)?
                .node
                .dirent
                .dirent_table()
                .ok_or(Error::IsNotDirectory)?
        };

        // A table's tree starts at its first entry, so padding there leaves it
        // empty, though a scan would read every sector to find that out
        let in_dir = |err: Error| err.in_directory(dir.clone());
        if dirtab.is_empty()
            || DirectoryEntryNode::read_from_disk(&mut self.dev, dirtab.offset(0)?)
                .map_err(in_dir)?
                .is_none()
        {
            return Ok(Vec::new());
        }

        let mut tree = dirtab.scan_dirent_tree(&mut self.dev).map_err(in_dir)?;
        let mut entries = Vec::new();
        while let Some(dirent) = tree.next_entry().map_err(in_dir)? {
            let name = dirent.name_str()?.into_owned();
            self.entries.insert(dir.join(&name), dirent);
            entries.push(FileEntry {
                name,
                file_type: if dirent.node.dirent.is_directory() {
                    FileType::Directory
                } else {
                    FileType::File
                },
                len: dirent.node.dirent.data.size as u64,
            });
        }

        // `dir_tree` would queue a repeated directory name twice, doubling the walk per level
        let mut names = BTreeSet::new();
        if let Some(entry) = entries.iter().find(|entry| !names.insert(&entry.name)) {
            return Err(Error::DuplicateName(entry.name.clone()));
        }

        // Only tables with entries count: empty ones may share an all-padding sector
        if !entries.is_empty() {
            let first_path = self
                .table_paths
                .overlapping_or_insert(&dirtab, || dir.clone());
            if first_path.is_some_and(|first_path| dir != first_path) {
                return Err(Error::Loop);
            }
        }

        Ok(entries)
    }
}

impl<D, W, Copier> FilesystemCopier<W> for XDVDFSFilesystem<D, W, Copier>
where
    D: BlockDeviceRead + Sized,
    W: BlockDeviceWrite + ?Sized,
    Copier: RWCopier<D, W>,
{
    fn copy_file_in(
        &mut self,
        src: &ImagePath,
        dest: &mut W,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        let dirent = self.lookup(src)?;

        let size_to_copy = core::cmp::min(size, dirent.node.dirent.data.size as u64);
        if size_to_copy == 0 {
            return Ok(0);
        }

        let input_offset = dirent.node.dirent.data.offset(input_offset)?;
        let copied = self
            .copier
            .copy(
                input_offset,
                output_offset,
                size_to_copy,
                &mut self.dev,
                dest,
            )
            .map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => Error::TruncatedFile(src.clone()),
                kind => io::Error::new(kind, format!("{src}: {err}")).into(),
            })?;
        if copied < size_to_copy {
            return Err(Error::TruncatedFile(src.clone()));
        }

        Ok(copied)
    }
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use crate::{
        Error,
        blockdev::BlockDeviceRead,
        layout::{
            DirectoryEntryDiskData, DirectoryEntryDiskNode, DirectoryEntryTable, DirentAttributes,
            DiskRegion, SECTOR_SIZE, SECTOR_SIZE_U64, SECTOR_SIZE_USZ, VolumeDescriptor,
        },
        read::read_volume,
        write::{
            fs::{
                FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, ImagePath,
                MemoryFilesystem, SectorLinearBlockDevice, SectorLinearBlockFilesystem,
                SectorLinearImage, dir_tree,
            },
            img::{NoOpProgressVisitor, create_xdvdfs_image},
        },
    };

    use super::{DefaultCopier, XDVDFSFilesystem};

    const VOLUME_SECTOR: u32 = 32;
    const ROOT_SECTOR: u32 = VOLUME_SECTOR + 1;

    fn sector_at(sector: u32) -> usize {
        sector as usize * SECTOR_SIZE_USZ
    }

    /// An 0xff-filled image of `sectors` sectors with a one-sector root table.
    fn blank_image(sectors: u32) -> Vec<u8> {
        let root = DirectoryEntryTable::new(SECTOR_SIZE, ROOT_SECTOR);
        let mut img = vec![0xff; sector_at(sectors)];
        let volume = VolumeDescriptor::new(root).to_bytes();
        img[sector_at(VOLUME_SECTOR)..][..volume.len()].copy_from_slice(&volume);
        img
    }

    /// Writes a directory entry at byte `at` and returns where the next one goes.
    fn put_dir(img: &mut [u8], at: usize, name: &str, data: DiskRegion) -> usize {
        let node = DirectoryEntryDiskNode {
            left_entry_offset: 0,
            right_entry_offset: 0,
            dirent: DirectoryEntryDiskData {
                data,
                attributes: DirentAttributes(0).with_directory(true),
                filename_length: name.len() as u8,
            },
        };
        img[at..][..0xe].copy_from_slice(&node.to_bytes());
        img[at + 0xe..][..name.len()].copy_from_slice(name.as_bytes());
        at + (0xe + name.len()).next_multiple_of(4)
    }

    /// An image whose table `i` sits at sector `ROOT_SECTOR + i`, root first.
    /// Each entry is a directory name and the index of the table it points at.
    fn image_with_tables(tables: &[&[(&str, u32)]]) -> Cursor<Vec<u8>> {
        let mut img = blank_image(ROOT_SECTOR + tables.len() as u32);
        for (index, entries) in tables.iter().enumerate() {
            let mut at = sector_at(ROOT_SECTOR + index as u32);
            for (name, table) in entries.iter() {
                let data = DiskRegion {
                    sector: ROOT_SECTOR + table,
                    size: SECTOR_SIZE,
                };
                at = put_dir(&mut img, at, name, data);
            }
        }

        Cursor::new(img)
    }

    fn open(img: Cursor<Vec<u8>>) -> XDVDFSFilesystem<Cursor<Vec<u8>>, [u8]> {
        XDVDFSFilesystem::new(img).expect("xdvdfs filesystem init should succeed")
    }

    #[test]
    fn test_read_dir_grandchild_loop() {
        let mut fs = open(image_with_tables(&[&[("a", 1)], &[("b", 1)]]));

        fs.read_dir(&ImagePath::root())
            .expect("Root read should succeed");
        fs.read_dir(&"/a".into()).expect("/a read should succeed");
        fs.read_dir(&"/a".into()).expect("/a reread should succeed");
        let res = fs.read_dir(&"/a/b".into());
        assert!(matches!(res, Err(Error::Loop)), "{res:?}");
    }

    #[test]
    fn test_read_dir_shared_table() {
        let mut fs = open(image_with_tables(&[
            &[("a", 1), ("b", 1)],
            &[("c", 2)],
            &[],
        ]));

        fs.read_dir(&ImagePath::root())
            .expect("Root read should succeed");
        fs.read_dir(&"/a".into()).expect("/a read should succeed");
        let res = fs.read_dir(&"/b".into());
        assert!(matches!(res, Err(Error::Loop)), "{res:?}");
    }

    #[test]
    fn test_read_dir_overlapping_tables() {
        let end = ROOT_SECTOR + 5;
        let mut img = blank_image(end);
        let mut at = sector_at(ROOT_SECTOR);
        for (name, sector) in ["a", "b", "c", "d"].into_iter().zip(ROOT_SECTOR + 1..) {
            let size = (end - sector) * SECTOR_SIZE;
            at = put_dir(&mut img, at, name, DiskRegion { sector, size });

            // Distinct names, since each table's scan runs over the later sectors too
            let leaf = DiskRegion { sector: 0, size: 0 };
            put_dir(&mut img, sector_at(sector), &format!("x{sector}"), leaf);
        }
        let mut fs = open(Cursor::new(img));

        fs.read_dir(&ImagePath::root())
            .expect("Root read should succeed");
        fs.read_dir(&"/a".into()).expect("/a read should succeed");
        let res = fs.read_dir(&"/b".into());
        assert!(matches!(res, Err(Error::Loop)), "{res:?}");
    }

    #[test]
    fn test_read_dir_entry_unreachable_by_search() {
        // Root lists `b` then `a`; a search for `a` goes left of `b` and finds nothing
        let mut fs = open(image_with_tables(&[
            &[("b", 1), ("a", 2)],
            &[("x", 3)],
            &[("y", 3)],
            &[],
        ]));
        let res = dir_tree(&mut fs, &mut |_| {});
        assert_eq!(res.map(|tree| tree.len()).map_err(|e| e.to_string()), Ok(5));
    }

    #[test]
    fn test_xdvdfs_fs_lookup_miss_names_path() {
        let mut fs = open(image_with_tables(&[&[("a", 1)], &[]]));
        let missing = ImagePath::from("/a/missing");

        let res = fs.read_dir(&missing).map(|entries| entries.len());
        assert!(
            matches!(&res, Err(Error::NotFound(path)) if *path == missing),
            "{res:?}"
        );

        let mut buf = [0u8; 1];
        let res = fs.copy_file_in(&missing, buf.as_mut_slice(), 0, 0, 1);
        let err = res.expect_err("copy of a missing file should fail");
        assert_eq!(err.to_string(), "path \"/a/missing\" not found");
    }

    #[test]
    fn test_read_dir_duplicate_name() {
        let doubled: Vec<[(&str, u32); 2]> = (1..=12).map(|next| [("a", next); 2]).collect();
        let mut tables: Vec<&[(&str, u32)]> = doubled.iter().map(|table| &table[..]).collect();
        tables.push(&[]);
        let mut fs = open(image_with_tables(&tables));

        let res = dir_tree(&mut fs, &mut |_| {});
        assert!(
            matches!(&res, Err(Error::DuplicateName(name)) if name == "a"),
            "{:?}",
            res.map(|tree| tree.len())
        );
    }

    /// An image that reads as padding past `data`, counting reads from `counted_from` on
    struct PaddedImage {
        data: Vec<u8>,
        counted_from: u64,
        counted: usize,
    }

    impl BlockDeviceRead for PaddedImage {
        fn read(&mut self, offset: u64, buffer: &mut [u8]) -> std::io::Result<()> {
            if offset >= self.counted_from {
                self.counted += 1;
            }

            buffer.fill(0xff);
            let data = self.data.get(offset as usize..).unwrap_or_default();
            let len = buffer.len().min(data.len());
            buffer[..len].copy_from_slice(&data[..len]);
            Ok(())
        }
    }

    #[test]
    fn test_read_dir_stops_at_empty_shared_table() {
        const DIRS: usize = 800;
        const TABLE_SECTORS: u32 = 8000;

        let mut memfs = MemoryFilesystem::default();
        for i in 0..DIRS {
            memfs.mkdir(format!("/{i:03}").as_str());
        }
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);
        let mut slbd = SectorLinearBlockDevice::default();
        create_xdvdfs_image(&mut slbfs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");
        let mut img = SectorLinearImage::new(&slbd, &mut slbfs)
            .read_linear(0, slbd.size())
            .expect("Image read should succeed");

        // Points every directory at one all-padding table past the end of `img`
        let shared = DiskRegion {
            sector: (img.len() / SECTOR_SIZE_USZ) as u32,
            size: TABLE_SECTORS * SECTOR_SIZE,
        };
        let root = read_volume(img.as_mut_slice())
            .expect("Volume should read")
            .root_table;
        let mut dirs = Vec::new();
        let mut scan = root
            .scan_dirent_tree(img.as_mut_slice())
            .expect("Root scan should succeed");
        while let Some(dirent) = scan.next_entry().expect("Root entry should read") {
            dirs.push(dirent);
        }
        assert_eq!(dirs.len(), DIRS);
        for dirent in dirs {
            let mut node = dirent.node;
            node.dirent.data = shared;
            img[dirent.offset as usize..][..0xe].copy_from_slice(&node.to_bytes());
        }

        let counted_from = (u64::from(shared.sector) + 1) * SECTOR_SIZE_U64;
        let dev = PaddedImage {
            data: img,
            counted_from,
            counted: 0,
        };
        let mut fs = XDVDFSFilesystem::<_, [u8]>::new(dev).expect("Volume should read");
        let tree = dir_tree(&mut fs, &mut |_| {}).expect("dir_tree should succeed");
        assert_eq!(tree.len(), DIRS + 1);
        assert_eq!(
            fs.dev.counted, 0,
            "reads past the shared table's first sector"
        );
    }

    #[test]
    fn test_xdvdfs_fs_read_dir_without_parent_first() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b/c", b"Hello World");
        memfs.create("/a/d", b"Goodbye World");
        let mut slbdfs = SectorLinearBlockFilesystem::new(memfs);
        let mut slbd = SectorLinearBlockDevice::default();
        create_xdvdfs_image(&mut slbdfs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");

        let img = SectorLinearImage::new(&slbd, &mut slbdfs);
        let mut fs = XDVDFSFilesystem::<_, [u8]>::new(img).expect("Volume should read");
        let dir = fs
            .read_dir(&"/a/b".into())
            .expect("/a/b read should succeed");
        assert_eq!(
            dir,
            &[FileEntry {
                name: "c".into(),
                file_type: FileType::File,
                len: 11,
            }]
        );
    }

    #[test]
    fn test_write_xdvdfs_invalid_volume() {
        let memfs = MemoryFilesystem::default();
        let mut slbdfs = SectorLinearBlockFilesystem::new(memfs);
        let slbd = SectorLinearBlockDevice::default();
        let img = SectorLinearImage::new(&slbd, &mut slbdfs);
        let fs = XDVDFSFilesystem::<_, [u8], DefaultCopier<_, _>>::new(img);

        assert!(matches!(fs, Err(Error::InvalidVolume)));
    }

    #[test]
    fn test_write_xdvdfs_hierarchy() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b/c", b"Hello World");
        memfs.create("/a/d", b"Goodbye World");
        let mut slbdfs = SectorLinearBlockFilesystem::new(memfs);
        let mut slbd = SectorLinearBlockDevice::default();

        create_xdvdfs_image(&mut slbdfs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");
        let img = SectorLinearImage::new(&slbd, &mut slbdfs);
        let mut fs = XDVDFSFilesystem::<_, [u8], DefaultCopier<_, _>>::new(img)
            .expect("xdvdfs filesystem init should succeed");

        let dir = fs
            .read_dir(&ImagePath::root())
            .expect("Root read should succeed");
        assert_eq!(
            dir,
            &[FileEntry {
                name: "a".into(),
                file_type: FileType::Directory,
                len: 2048,
            },]
        );

        let dir = fs.read_dir(&"/a".into()).expect("/a read should succeed");
        assert_eq!(
            dir,
            &[
                FileEntry {
                    name: "b".into(),
                    file_type: FileType::Directory,
                    len: 2048,
                },
                FileEntry {
                    name: "d".into(),
                    file_type: FileType::File,
                    len: 13,
                },
            ]
        );

        let dir = fs
            .read_dir(&"/a/b".into())
            .expect("/a/b read should succeed");
        assert_eq!(
            dir,
            &[FileEntry {
                name: "c".into(),
                file_type: FileType::File,
                len: 11,
            },]
        );
    }

    #[test]
    fn test_write_xdvdfs_copier() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b/c", b"Hello World");
        memfs.create("/a/d", b"Goodbye World");
        let mut slbdfs = SectorLinearBlockFilesystem::new(memfs);
        let mut slbd = SectorLinearBlockDevice::default();

        create_xdvdfs_image(&mut slbdfs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");
        let img = SectorLinearImage::new(&slbd, &mut slbdfs);
        let mut fs = XDVDFSFilesystem::<_, [u8], DefaultCopier<_, _>>::new(img)
            .expect("xdvdfs filesystem init should succeed");

        let mut buf = [0u8; 9];
        let copied = fs.copy_file_in(&"/a/b/c".into(), &mut buf, 4, 2, 7);
        assert_eq!(copied.expect("Copy should succeed"), 7);
        assert_eq!(&buf, b"\0\0o World");
    }
}

use core::fmt::Display;
use std::fs::{self, DirEntry, File};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::blockdev::{BlockDeviceWrite, NullBlockDevice};

use super::{FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, PathRef};

pub struct StdFilesystem {
    root: PathBuf,
}

fn path_error(path: &Path, kind: ErrorKind, what: impl Display) -> io::Error {
    io::Error::new(kind, format!("{}: {what}", path.display()))
}

fn short_file_error(path: &Path) -> io::Error {
    path_error(
        path,
        ErrorKind::UnexpectedEof,
        "file is shorter than its listed size",
    )
}

fn open_at(path: &Path, offset: u64) -> io::Result<File> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(file)
}

impl StdFilesystem {
    pub fn create(root: &Path) -> Self {
        Self {
            root: root.to_owned(),
        }
    }

    fn file_entry(&self, dir: &Path, entry: DirEntry) -> io::Result<FileEntry> {
        let path = entry.path();
        let metadata = fs::metadata(&path).map_err(|err| path_error(&path, err.kind(), err))?;
        let name = entry.file_name().into_string().map_err(|_| {
            path_error(
                &path,
                ErrorKind::InvalidData,
                "name isn't valid Unicode; rename it",
            )
        })?;

        let (file_type, len) = if metadata.is_file() {
            (FileType::File, metadata.len())
        } else if metadata.is_dir() {
            let link_type = entry
                .file_type()
                .map_err(|err| path_error(&path, err.kind(), err))?;
            if link_type.is_symlink() {
                self.check_symlink_loop(dir, &path)?;
            }
            (FileType::Directory, 0)
        } else {
            return Err(path_error(
                &path,
                ErrorKind::Unsupported,
                "not a regular file or directory",
            ));
        };

        Ok(FileEntry {
            name,
            file_type,
            len,
        })
    }

    /// Errors if directory symlink `path`, listed in `dir`, points at `dir`,
    /// at a directory on the image path from the root to `dir`, or above one
    /// of those, where listing would never end.
    fn check_symlink_loop(&self, dir: &Path, path: &Path) -> io::Result<()> {
        let target = fs::canonicalize(path).map_err(|err| path_error(path, err.kind(), err))?;
        for ancestor in dir
            .ancestors()
            .take_while(|parent| parent.starts_with(&self.root))
        {
            let ancestor =
                fs::canonicalize(ancestor).map_err(|err| path_error(ancestor, err.kind(), err))?;
            if ancestor.starts_with(&target) {
                let what = format!("symlink loop back to {}", target.display());
                return Err(path_error(path, ErrorKind::Other, what));
            }
        }

        Ok(())
    }
}

impl FilesystemHierarchy for StdFilesystem {
    type Error = io::Error;

    fn read_dir(&mut self, dir: PathRef<'_>) -> Result<Vec<FileEntry>, io::Error> {
        let dir = dir.as_path_buf(&self.root);
        fs::read_dir(&dir)
            .map_err(|err| path_error(&dir, err.kind(), err))?
            .map(|entry| {
                let entry = entry.map_err(|err| path_error(&dir, err.kind(), err))?;
                self.file_entry(&dir, entry)
            })
            .collect()
    }
}

impl<T> FilesystemCopier<T> for StdFilesystem
where
    T: io::Write + Seek + BlockDeviceWrite<WriteError = io::Error>,
{
    type Error = io::Error;

    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut T,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, io::Error> {
        dest.seek(SeekFrom::Start(output_offset))?;

        let src = src.as_path_buf(&self.root);
        let file = open_at(&src, input_offset).map_err(|err| path_error(&src, err.kind(), err))?;
        let mut file = io::BufReader::with_capacity(1024 * 1024, file.take(size));
        let copied = io::copy(&mut file, dest).map_err(|err| path_error(&src, err.kind(), err))?;
        if copied < size {
            return Err(short_file_error(&src));
        }

        Ok(copied)
    }
}

impl FilesystemCopier<[u8]> for StdFilesystem {
    type Error = io::Error;

    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut [u8],
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, io::Error> {
        let src = src.as_path_buf(&self.root);
        let output_offset = output_offset as usize;
        let size = core::cmp::min(size as usize, <[u8]>::len(dest) - output_offset);
        let dest = &mut dest[output_offset..(output_offset + size)];

        open_at(&src, input_offset)
            .and_then(|mut file| file.read_exact(dest))
            .map_err(|err| match err.kind() {
                ErrorKind::UnexpectedEof => short_file_error(&src),
                kind => path_error(&src, kind, err),
            })?;
        Ok(size as u64)
    }
}

impl FilesystemCopier<NullBlockDevice> for StdFilesystem {
    type Error = core::convert::Infallible;

    fn copy_file_in(
        &mut self,
        _src: PathRef<'_>,
        dest: &mut NullBlockDevice,
        _input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, core::convert::Infallible> {
        dest.write_size_adjustment(output_offset, size);
        Ok(size)
    }
}

#[cfg(test)]
mod test {
    use std::io::{Cursor, Write};

    use crate::{
        blockdev::NullBlockDevice,
        write::fs::{
            FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, SectorLinearBlockDevice,
            SectorLinearBlockFilesystem, SectorLinearImage,
        },
    };

    use super::StdFilesystem;

    #[test]
    fn test_write_stdfs_read_dir() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");

        std::fs::create_dir(tempdir.path().join("subdir")).expect("Failed to create dir direntry");
        std::fs::create_dir(tempdir.path().join("subdir/entry"))
            .expect("Failed to create dir direntry");

        {
            let mut file = std::fs::File::create(tempdir.path().join("file"))
                .expect("Failed to create file direntry");
            file.write_all(b"Hello World")
                .expect("Failed to write to file entry");
        }

        let mut stdfs = StdFilesystem::create(tempdir.path());

        let mut result = stdfs.read_dir("/".into()).expect("read_dir should succeed");

        // Filesystem ordering is non-deterministic,
        // so sort the records by name.
        result.sort();
        assert_eq!(
            result,
            &[
                FileEntry {
                    name: "file".into(),
                    file_type: FileType::File,
                    len: 11,
                },
                FileEntry {
                    name: "subdir".into(),
                    file_type: FileType::Directory,
                    len: 0,
                },
            ],
        );

        let result = stdfs
            .read_dir("/subdir".into())
            .expect("read_dir should succeed");
        assert_eq!(
            result,
            &[FileEntry {
                name: "entry".into(),
                file_type: FileType::Directory,
                len: 0,
            },],
        );
    }

    #[test]
    fn test_write_stdfs_io_write_copier() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");

        std::fs::create_dir(tempdir.path().join("subdir")).expect("Failed to create dir direntry");

        {
            let mut file = std::fs::File::create(tempdir.path().join("subdir/file"))
                .expect("Failed to create file direntry");
            file.write_all(b"Hello World")
                .expect("Failed to write to file entry");
        }

        let mut stdfs = StdFilesystem::create(tempdir.path());
        let mut cursor = Cursor::new(Vec::<u8>::new());

        let result = stdfs.copy_file_in(
            "subdir/file".into(),
            &mut cursor,
            /*input_offset=*/ 1,
            /*output_offset=*/ 2,
            /*size=*/ 10,
        );
        assert!(result.is_ok_and(|len| len == 10));
        assert_eq!(cursor.get_ref(), b"\0\0ello World");
    }

    #[test]
    fn test_write_stdfs_byte_slice_copier() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");

        std::fs::create_dir(tempdir.path().join("subdir")).expect("Failed to create dir direntry");

        {
            let mut file = std::fs::File::create(tempdir.path().join("subdir/file"))
                .expect("Failed to create file direntry");
            file.write_all(b"Hello World")
                .expect("Failed to write to file entry");
        }

        let mut stdfs = StdFilesystem::create(tempdir.path());
        let mut buffer = [0u8; 13];

        let result = stdfs.copy_file_in(
            "subdir/file".into(),
            buffer.as_mut_slice(),
            /*input_offset=*/ 1,
            /*output_offset=*/ 2,
            /*size=*/ 10,
        );
        assert!(result.is_ok_and(|len| len == 10));
        assert_eq!(&buffer, b"\0\0ello World\0");
    }

    #[test]
    fn test_write_stdfs_null_copier() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");

        std::fs::create_dir(tempdir.path().join("subdir")).expect("Failed to create dir direntry");

        {
            let mut file = std::fs::File::create(tempdir.path().join("subdir/file"))
                .expect("Failed to create file direntry");
            file.write_all(b"Hello World")
                .expect("Failed to write to file entry");
        }

        let mut stdfs = StdFilesystem::create(tempdir.path());
        let mut nullbd = NullBlockDevice::default();

        let result = stdfs.copy_file_in(
            "subdir/file".into(),
            &mut nullbd,
            /*input_offset=*/ 1,
            /*output_offset=*/ 2,
            /*size=*/ 10,
        );
        assert!(result.is_ok_and(|len| len == 10));
        assert_eq!(nullbd.len_blocking(), 12);
    }

    #[test]
    fn test_write_stdfs_io_write_copier_short_file_errors() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
        let path = tempdir.path().join("file");
        std::fs::write(&path, b"Hello World").expect("Failed to write file");

        let mut stdfs = StdFilesystem::create(tempdir.path());
        let mut cursor = Cursor::new(Vec::<u8>::new());

        let result = stdfs.copy_file_in(
            "file".into(),
            &mut cursor,
            /*input_offset=*/ 0,
            /*output_offset=*/ 0,
            /*size=*/ 16,
        );
        let err = result.expect_err("copy past the end of the file should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "{err}"
        );
    }

    #[test]
    fn test_write_stdfs_byte_slice_copier_short_file_errors() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
        let path = tempdir.path().join("file");
        std::fs::write(&path, b"Hello World").expect("Failed to write file");

        let mut stdfs = StdFilesystem::create(tempdir.path());
        let mut buffer = [0u8; 16];

        let result = stdfs.copy_file_in(
            "file".into(),
            buffer.as_mut_slice(),
            /*input_offset=*/ 0,
            /*output_offset=*/ 0,
            /*size=*/ 16,
        );
        let err = result.expect_err("copy past the end of the file should fail");
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "{err}"
        );
    }

    /// Reads the linear image of `dir/file`, holding `data` but listed as `listed_len` bytes
    fn linear_image_read(
        dir: &std::path::Path,
        data: &[u8],
        listed_len: u64,
    ) -> std::io::Result<Vec<u8>> {
        std::fs::write(dir.join("file"), data).expect("Failed to write file");

        let mut slbfs = SectorLinearBlockFilesystem::new(StdFilesystem::create(dir));
        let mut slbd = SectorLinearBlockDevice::default();
        slbfs
            .copy_file_in("/file".into(), &mut slbd, 0, 0, listed_len)
            .expect("Adding the file should succeed");

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let len = listed_len.next_multiple_of(crate::layout::SECTOR_SIZE_U64);
        image.read_linear(0, len)
    }

    #[test]
    fn test_write_stdfs_linear_image_pads_last_sector() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
        let data = linear_image_read(tempdir.path(), &[7; 100], 100).expect("Read should succeed");
        assert_eq!(data.len(), crate::layout::SECTOR_SIZE_USZ);
        assert_eq!(data[..100], [7; 100]);
        assert!(data[100..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn test_write_stdfs_linear_image_short_file_errors() {
        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
        let Err(err) = linear_image_read(tempdir.path(), &[7; 100], 3000) else {
            panic!("Read past the end of the file should fail");
        };
        let path = tempdir.path().join("file");
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "{err}"
        );
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[cfg(unix)]
    #[test]
    fn test_stdfs_follows_file_and_dir_symlinks() {
        use std::os::unix::fs::symlink;

        let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
        let src = tempdir.path().join("src");
        std::fs::create_dir(&src).expect("Failed to create src");
        std::fs::create_dir(tempdir.path().join("dir")).expect("Failed to create dir");
        std::fs::write(tempdir.path().join("file"), b"Hello World").expect("Failed to write file");
        symlink("../file", src.join("file_link")).expect("Failed to link file");
        symlink("../dir", src.join("dir_link")).expect("Failed to link dir");

        let mut stdfs = StdFilesystem::create(&src);
        let mut result = stdfs.read_dir("/".into()).expect("read_dir should succeed");
        result.sort();
        assert_eq!(
            result,
            &[
                FileEntry {
                    name: "dir_link".into(),
                    file_type: FileType::Directory,
                    len: 0,
                },
                FileEntry {
                    name: "file_link".into(),
                    file_type: FileType::File,
                    len: 11,
                },
            ],
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_stdfs_dir_symlink_loop_errors() {
        use crate::write::fs::dir_tree;
        use std::os::unix::fs::symlink;

        /// Lists a root holding `links`, and returns the error with the root's
        /// path replaced by `<root>`.
        fn dir_tree_error(links: &[(&str, &str)]) -> String {
            let tempdir = tempfile::tempdir().expect("Failed to create tempdir");
            let root = tempdir.path().join("src");
            for (link, target) in links {
                let link = root.join(link);
                std::fs::create_dir_all(link.parent().unwrap()).expect("Failed to create dir");
                symlink(target, link).expect("Failed to link");
            }

            let mut stdfs = StdFilesystem::create(&root);
            let res = dir_tree(&mut stdfs, &mut |_| {});
            res.expect_err("dir_tree should fail")
                .to_string()
                .replace(&root.display().to_string(), "<root>")
        }

        let err = dir_tree_error(&[("sub/loop", "..")]);
        assert!(err.starts_with("<root>/sub/loop: symlink loop"), "{err}");

        let err = dir_tree_error(&[("a/l", "../b"), ("b/l", "../a")]);
        assert!(
            err.starts_with("<root>/a/l/l: symlink loop")
                || err.starts_with("<root>/b/l/l: symlink loop"),
            "{err}"
        );

        // Fails at the first hop, before listing the root's parent
        let err = dir_tree_error(&[("up", "..")]);
        assert!(err.starts_with("<root>/up: symlink loop"), "{err}");
    }
}

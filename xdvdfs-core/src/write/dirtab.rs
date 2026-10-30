use crate::Error;
use crate::layout::{
    self, DirectoryEntryData, DirectoryEntryDiskNode, DirentAttributes, NameError, is_safe_name,
};
use crate::write::fs::ImagePath;

use super::sector::{SectorAllocator, required_sectors};

#[derive(Default)]
pub struct DirectoryEntryTableBuilder<'alloc> {
    /// The directory this table lists, to name entries in errors
    dir: ImagePath,
    entries: Vec<DirectoryEntryData<'alloc>>,
}

struct TreeNode<'alloc> {
    entry: DirectoryEntryData<'alloc>,
    left: Option<usize>,
    right: Option<usize>,
}

/// Writer for directory entry tables
pub struct DirectoryEntryTableWriter<'alloc> {
    /// A balanced search tree in preorder, the order entries are laid out in the table
    nodes: Vec<TreeNode<'alloc>>,
    /// Each node's byte offset in the table
    offsets: Vec<u32>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct FileListingEntry<'alloc> {
    pub name: &'alloc str,
    pub sector: u64,
    pub size: u64,
    pub is_dir: bool,
    pub idx: usize,
}

/// Returns alignment needed to ensure an entry at a given sector offset and size
/// does not cross a sector boundary. If it does, the offset is aligned to the next sector.
fn sector_align(offset: u32, size: u32) -> u32 {
    let used_sectors = required_sectors(offset as u64);
    let needed_sectors = required_sectors(offset as u64 + size as u64);

    // If the offset already lands on a sector boundary, there is nothing to be done.
    // If we need more sectors to contain the data (offset + size) than everything up
    // to offset, then we know the data crosses a sector boundary.
    if !offset.is_multiple_of(layout::SECTOR_SIZE) && needed_sectors > used_sectors {
        offset.next_multiple_of(layout::SECTOR_SIZE) - offset
    } else {
        0
    }
}

/// Compute each entry's offset from the start of the table, given each
/// entry's length on disk. The offset is a partial sum of entry lengths.
fn compute_offsets(lens: impl Iterator<Item = u32>) -> impl Iterator<Item = u32> {
    lens.scan(0u32, |state, len| {
        // If this entry does not fit at its offset, place it on a new sector
        *state += sector_align(*state, len);
        let curr_offset = *state;

        *state += len;
        Some(curr_offset)
    })
}

fn child_offset(index: Option<usize>, offsets: &[u32]) -> crate::Result<u16> {
    index
        .map(|idx| offsets[idx] / 4)
        .unwrap_or_default()
        .try_into()
        .map_err(|_| Error::TooManyDirectoryEntries)
}

fn dirent_data_to_disk_node(
    data: &DirectoryEntryData,
    left_child_index: Option<usize>,
    right_child_index: Option<usize>,
    offsets: &[u32],
    sector: u32,
) -> crate::Result<DirectoryEntryDiskNode> {
    let mut dirent = data.node;

    let left_entry_offset = child_offset(left_child_index, offsets)?;
    let right_entry_offset = child_offset(right_child_index, offsets)?;
    dirent.data.sector = sector;

    Ok(DirectoryEntryDiskNode {
        left_entry_offset,
        right_entry_offset,
        dirent,
    })
}

fn serialize_dirent_disk_node(table: &mut [u8], dirent: DirectoryEntryDiskNode, name_bytes: &[u8]) {
    table[..0xe].copy_from_slice(&dirent.to_bytes());

    let name_len = dirent.dirent.filename_length as usize;
    table[0xe..(0xe + name_len)].copy_from_slice(&name_bytes[0..name_len]);
}

/// Appends a balanced tree of `sorted` to `nodes` in preorder,
/// and returns the index of its root
fn push_subtree<'alloc>(
    sorted: &[DirectoryEntryData<'alloc>],
    nodes: &mut Vec<TreeNode<'alloc>>,
) -> Option<usize> {
    let mid = sorted.len() / 2;
    let idx = nodes.len();
    nodes.push(TreeNode {
        entry: sorted.get(mid)?.clone(),
        left: None,
        right: None,
    });

    nodes[idx].left = push_subtree(&sorted[..mid], nodes);
    nodes[idx].right = push_subtree(&sorted[mid + 1..], nodes);
    Some(idx)
}

impl<'alloc> DirectoryEntryTableBuilder<'alloc> {
    pub fn new(dir: ImagePath) -> Self {
        Self {
            dir,
            entries: Vec::new(),
        }
    }

    fn add_node(
        &mut self,
        name: &'alloc str,
        size: u32,
        attributes: DirentAttributes,
        idx: usize,
    ) -> crate::Result<()> {
        let path = || self.dir.join(name);
        if !is_safe_name(name) {
            return Err(Error::InvalidFileName(path()));
        }

        let dirent = match DirectoryEntryData::new_without_sector(name, size, attributes, idx) {
            Ok(dirent) => dirent,
            Err(NameError::TooLong) => return Err(Error::FileNameTooLong(path())),
            Err(NameError::NotEncodable) => return Err(Error::FileNameNotEncodable(path())),
        };
        self.entries.push(dirent);
        Ok(())
    }

    pub fn add_dir(&mut self, name: &'alloc str, size: u32, idx: usize) -> crate::Result<()> {
        let attributes = DirentAttributes(0).with_directory(true);

        let size = size + ((2048 - size % 2048) % 2048);
        self.add_node(name, size, attributes, idx)
    }

    pub fn add_file(&mut self, name: &'alloc str, size: u32) -> crate::Result<()> {
        let attributes = DirentAttributes(0).with_archive(true);
        self.add_node(name, size, attributes, 0)
    }

    pub fn reserve(&mut self, size: usize) {
        self.entries.reserve(size);
    }

    pub fn build(mut self) -> crate::Result<DirectoryEntryTableWriter<'alloc>> {
        self.entries.sort();
        if let Some([_, later]) = self.entries.array_windows().find(|[a, b]| a.cmp(b).is_eq()) {
            return Err(Error::DuplicateFileName(self.dir.join(later.get_name())));
        }

        let mut nodes = Vec::with_capacity(self.entries.len());
        push_subtree(&self.entries, &mut nodes);
        Ok(DirectoryEntryTableWriter::new(nodes))
    }
}

impl<'alloc> DirectoryEntryTableWriter<'alloc> {
    fn new(nodes: Vec<TreeNode<'alloc>>) -> Self {
        let offsets = compute_offsets(nodes.iter().map(|node| node.entry.len_on_disk())).collect();
        Self { nodes, offsets }
    }

    /// Returns the size of the directory entry table, in bytes.
    pub fn dirtab_size(&self) -> u32 {
        let last = self.nodes.last().zip(self.offsets.last());
        // FS bug: zero sized dirents are listed as size 2048
        last.map_or(2048, |(node, offset)| offset + node.entry.len_on_disk())
    }

    /// Iterate over the FileListingEntries within the directory entry table.
    /// Sector will be zero until it is computed by `disk_repr`.
    pub fn iter(&self) -> impl Iterator<Item = FileListingEntry<'_>> {
        self.nodes
            .iter()
            .map(|TreeNode { entry, .. }| FileListingEntry {
                name: entry.get_name(),
                sector: entry.node.data.sector as u64,
                size: entry.node.data.size as u64,
                is_dir: entry.node.is_directory(),
                idx: entry.idx,
            })
    }

    /// Allocates sectors for the table's entries and serializes the table
    /// into `dirtab_bytes`, which can be reused across tables.
    pub fn disk_repr(
        &mut self,
        allocator: &mut SectorAllocator,
        dirtab_bytes: &mut Vec<u8>,
    ) -> crate::Result<()> {
        let size = self.dirtab_size().next_multiple_of(layout::SECTOR_SIZE) as usize;

        dirtab_bytes.fill(0xff);
        dirtab_bytes.resize(size, 0xff);

        for (node, &offset) in self.nodes.iter_mut().zip(&self.offsets) {
            let sector = allocator.allocate_contiguous(node.entry.node.data.size as u64);
            node.entry.node.data.sector = sector;

            let dirent = dirent_data_to_disk_node(
                &node.entry,
                node.left,
                node.right,
                &self.offsets,
                sector,
            )?;
            serialize_dirent_disk_node(
                &mut dirtab_bytes[offset as usize..],
                dirent,
                node.entry.get_encoded_name(),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{
        Error,
        layout::DirectoryEntryDiskNode,
        write::{dirtab::FileListingEntry, fs::ImagePath, sector::SectorAllocator},
    };

    use rand::{SeedableRng, rngs::StdRng, seq::SliceRandom};

    use super::{
        DirectoryEntryTableBuilder, TreeNode, child_offset, compute_offsets,
        dirent_data_to_disk_node, sector_align, serialize_dirent_disk_node,
    };
    use crate::layout::cmp_ignore_case_utf8;

    const SHUFFLE_SEED: u64 = 0x5842_4f58_5842_4f58;

    #[test]
    fn test_sector_align_offset_aligned() {
        assert_eq!(sector_align(0, 20), 0);
        assert_eq!(sector_align(2048, 20), 0);
    }

    #[test]
    fn test_sector_align_offset_contained_in_sector() {
        assert_eq!(sector_align(1000, 50), 0);
        assert_eq!(sector_align(2000, 20), 0);
    }

    #[test]
    fn test_sector_align_crosses_boundary() {
        assert_eq!(sector_align(2040, 20), 8);
        assert_eq!(sector_align(2020, 40), 28);
    }

    #[test]
    fn test_compute_offsets_single_sector() {
        let offsets: Vec<u32> = compute_offsets([100, 100, 100, 100].into_iter()).collect();
        assert_eq!(offsets, &[0, 100, 200, 300,]);
    }

    #[test]
    fn test_compute_offsets_multiple_sectors() {
        let offsets: Vec<u32> = compute_offsets([252; 10].into_iter()).collect();
        assert_eq!(
            offsets,
            &[
                0, 252, 504, 756, 1008, 1260, 1512, 1764,
                // 252 no longer fits in sector, push to 2048
                2048, 2300,
            ]
        );
    }

    #[test]
    fn test_child_offset_leaf() {
        let offsets = &[0, 100, 200];
        let offset = child_offset(None, offsets).expect("Offset should fit");
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_child_offset_node() {
        let offsets = &[0, 100, 200];
        let offset = child_offset(Some(1), offsets).expect("Offset should fit");
        assert_eq!(offset, 25);
    }

    #[test]
    fn test_child_offset_out_of_range() {
        let offsets = &[0, 100, 262144];
        assert!(matches!(
            child_offset(Some(2), offsets),
            Err(Error::TooManyDirectoryEntries)
        ));
    }

    #[test]
    fn test_dirent_data_to_disk_node_leaf_node() {
        use crate::layout::{DirectoryEntryData, DirentAttributes};

        let data = DirectoryEntryData::new_without_sector(
            "HelloWorld",
            2048,
            DirentAttributes(0).with_directory(true),
            1234,
        )
        .expect("Data should be valid");
        let offsets = &[0, 2048, 4096];

        let node = dirent_data_to_disk_node(&data, None, None, offsets, 33)
            .expect("Node should be created without error");
        assert_eq!(node.left_entry_offset, 0);
        assert_eq!(node.right_entry_offset, 0);
        assert_eq!(node.dirent.data.sector, 33);
        assert_eq!(node.dirent.filename_length, 10);
        assert_eq!(data.get_encoded_name(), b"HelloWorld");
    }

    #[test]
    fn test_dirent_data_to_disk_node_with_child_nodes() {
        use crate::layout::{DirectoryEntryData, DirentAttributes};

        let data = DirectoryEntryData::new_without_sector(
            "HelloWorld",
            2048,
            DirentAttributes(0).with_directory(true),
            1234,
        )
        .expect("Data should be valid");
        let offsets = &[0, 2048, 4096];

        let node = dirent_data_to_disk_node(&data, Some(1), Some(2), offsets, 33)
            .expect("Node should be created without error");
        assert_eq!(node.left_entry_offset, 512);
        assert_eq!(node.right_entry_offset, 1024);
        assert_eq!(node.dirent.data.sector, 33);
        assert_eq!(node.dirent.filename_length, 10);
        assert_eq!(data.get_encoded_name(), b"HelloWorld");
    }

    #[test]
    fn test_serialize_dirent_disk_node_without_alignment() {
        use crate::layout::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};
        let node = DirectoryEntryDiskNode {
            left_entry_offset: 512,
            right_entry_offset: 1024,
            dirent: DirectoryEntryDiskData {
                data: DiskRegion {
                    sector: 33,
                    size: 2048,
                },
                attributes: DirentAttributes(0).with_directory(true),
                filename_length: 10,
            },
        };
        let name_bytes = "HelloWorld".as_bytes();

        // Initial len + sizeof(node) + sizeof(name)
        let mut buffer = vec![0xff; 2048 + 0xe + 10];
        serialize_dirent_disk_node(&mut buffer[2048..], node, name_bytes);
        assert_eq!(&buffer[0..2048], vec![0xff; 2048]);
        assert_eq!(buffer[2048..(2048 + 0xe)], node.to_bytes());
        assert_eq!(&buffer[(2048 + 0xe)..], name_bytes);
    }

    #[test]
    fn test_serialize_dirent_disk_node_with_alignment() {
        use crate::layout::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};
        let node = DirectoryEntryDiskNode {
            left_entry_offset: 512,
            right_entry_offset: 1024,
            dirent: DirectoryEntryDiskData {
                data: DiskRegion {
                    sector: 33,
                    size: 2048,
                },
                attributes: DirentAttributes(0).with_directory(true),
                filename_length: 11,
            },
        };
        let name_bytes = "HelloWorlds".as_bytes();

        // Initial len + sizeof(node) + sizeof(name) + 4-byte alignment
        let mut buffer = vec![0xff; 2048 + 0xe + 11 + 3];
        serialize_dirent_disk_node(&mut buffer[2048..], node, name_bytes);
        assert_eq!(&buffer[0..2048], vec![0xff; 2048]);
        assert_eq!(buffer[2048..(2048 + 0xe)], node.to_bytes());
        assert_eq!(&buffer[(2048 + 0xe)..(2048 + 0xe + 11)], name_bytes);
        assert_eq!(&buffer[(2048 + 0xe + 11)..], &[0xff, 0xff, 0xff]);
    }

    #[test]
    fn test_dirtab_writer_empty_size_computation() {
        let writer = DirectoryEntryTableBuilder::default();
        let writer = writer.build().unwrap();

        assert_eq!(writer.dirtab_size(), 2048);
    }

    #[test]
    fn test_dirtab_writer_single_directory_size_computation() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_dir("test", 30, 1234).unwrap();

        let writer = writer.build().unwrap();
        assert_eq!(writer.dirtab_size(), 0xe + 4 + 2);
    }

    #[test]
    fn test_dirtab_writer_single_file_size_computation() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_file("test", 30).unwrap();

        let writer = writer.build().unwrap();
        assert_eq!(writer.dirtab_size(), 0xe + 4 + 2);
    }

    #[test]
    fn test_dirtab_writer_multiple_entry_size_computation_without_realignment() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_file("file", 30).unwrap();
        writer.add_dir("dir", 30, 1234).unwrap();

        let writer = writer.build().unwrap();

        let dirent_len = 2 * 0xe;
        let filename_len = 4 + 3;
        let padding = 2 + 3;
        assert_eq!(writer.dirtab_size(), dirent_len + filename_len + padding);
    }

    #[test]
    fn test_dirtab_writer_multiple_entry_size_computation_with_realignment() {
        let mut writer = DirectoryEntryTableBuilder::default();

        // Add 103 files with name length '6'
        // Each dirent is of length 20, with a total over 2048
        let names: Vec<String> = (0..103).into_iter().map(|i| format!("{i:06}")).collect();
        for name in &names {
            writer.add_file(name, 10).unwrap();
        }

        let writer = writer.build().unwrap();

        let dirent_len = 103 * 0xe;
        let filename_len = 103 * 6;
        let alignment = 8; // 102 * 20 = 2040, align to 2048
        assert_eq!(writer.dirtab_size(), dirent_len + filename_len + alignment);
    }

    #[test]
    fn test_dirtab_writer_serialize_empty_directory() {
        let writer = DirectoryEntryTableBuilder::default();
        let mut writer = writer.build().unwrap();
        let mut allocator = SectorAllocator::default();

        let mut dirtab_bytes = Vec::new();
        let res = writer.disk_repr(&mut allocator, &mut dirtab_bytes);
        assert!(res.is_ok());

        // Empty tables are '0xff' filled
        assert_eq!(dirtab_bytes, vec![0xffu8; 2048]);
        assert!(writer.iter().next().is_none());
    }

    #[test]
    fn test_dirtab_writer_serialize_single_file() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_file("test", 10).unwrap();

        let mut writer = writer.build().unwrap();
        let mut allocator = SectorAllocator::default();

        let mut dirtab_bytes = Vec::new();
        let res = writer.disk_repr(&mut allocator, &mut dirtab_bytes);
        assert!(res.is_ok());

        assert_eq!(
            &dirtab_bytes[0..0xe],
            &[0, 0, 0, 0, 33, 0, 0, 0, 10, 0, 0, 0, 32, 4,]
        );
        assert_eq!(&dirtab_bytes[0xe..(0xe + 4)], "test".as_bytes());
        assert_eq!(&dirtab_bytes[(0xe + 4)..], &vec![0xff; 2030]);

        let file_listing: Vec<_> = writer.iter().collect();
        assert_eq!(
            file_listing,
            &[FileListingEntry {
                name: "test",
                sector: 33,
                size: 10,
                is_dir: false,
                idx: 0,
            },]
        );
    }

    #[test]
    fn test_dirtab_writer_serialize_single_dir() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_dir("test", 20, 1234).unwrap();

        let mut writer = writer.build().unwrap();
        let mut allocator = SectorAllocator::default();
        let mut dirtab_bytes = Vec::new();
        let res = writer.disk_repr(&mut allocator, &mut dirtab_bytes);
        assert!(res.is_ok());

        assert_eq!(
            &dirtab_bytes[0..0xe],
            &[
                0, 0, 0, 0, 33, 0, 0, 0, 0, 8, 0, 0, // Dir is padded up to 2048 in size
                16, 4,
            ]
        );
        assert_eq!(&dirtab_bytes[0xe..(0xe + 4)], "test".as_bytes());
        assert_eq!(&dirtab_bytes[(0xe + 4)..], &vec![0xff; 2030]);

        let file_listing: Vec<_> = writer.iter().collect();
        assert_eq!(
            file_listing,
            &[FileListingEntry {
                name: "test",
                sector: 33,
                size: 2048,
                is_dir: true,
                idx: 1234,
            },]
        );
    }

    #[test]
    fn test_dirtab_writer_serialize_tree_entries() {
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_dir("t1", 20, 1).unwrap();
        writer.add_dir("t2", 20, 2).unwrap();
        writer.add_dir("t3", 20, 3).unwrap();

        let mut writer = writer.build().unwrap();
        let mut allocator = SectorAllocator::default();
        let mut dirtab_bytes = Vec::new();
        let res = writer.disk_repr(&mut allocator, &mut dirtab_bytes);
        assert!(res.is_ok());

        let entry_size: usize = 0xe + 2;

        assert_eq!(&dirtab_bytes[0..4], &[4, 0, 8, 0]);
        assert_eq!(&dirtab_bytes[entry_size..(entry_size + 4)], &[0, 0, 0, 0]);
        assert_eq!(
            &dirtab_bytes[(2 * entry_size)..(2 * entry_size + 4)],
            &[0, 0, 0, 0]
        );

        let file_listing: Vec<_> = writer.iter().collect();
        assert_eq!(
            file_listing,
            &[
                FileListingEntry {
                    name: "t2",
                    sector: 33,
                    size: 2048,
                    is_dir: true,
                    idx: 2,
                },
                FileListingEntry {
                    name: "t1",
                    sector: 34,
                    size: 2048,
                    is_dir: true,
                    idx: 1,
                },
                FileListingEntry {
                    name: "t3",
                    sector: 35,
                    size: 2048,
                    is_dir: true,
                    idx: 3,
                },
            ]
        );
    }

    #[test]
    fn test_dirtab_writer_serialize_entry_sector_alignment() {
        let mut writer = DirectoryEntryTableBuilder::default();

        // Add 103 files with name length '6'
        // Each dirent is of length 20, with a total over 2048
        let names: Vec<String> = (0..103).into_iter().map(|i| format!("{i:06}")).collect();
        for name in &names {
            writer.add_file(name, 10).unwrap();
        }

        let mut writer = writer.build().unwrap();
        let mut allocator = SectorAllocator::default();
        let mut dirtab_bytes = Vec::new();
        let res = writer.disk_repr(&mut allocator, &mut dirtab_bytes);
        assert!(res.is_ok());

        let entry_size: usize = 0xe + 6;
        let aligned_entry_offset: usize = entry_size * 102;
        assert_eq!(
            &dirtab_bytes[aligned_entry_offset..(aligned_entry_offset + 8)],
            &vec![0xff; 8]
        );
    }

    /// Checks the subtree at `idx` is balanced, appends its names in order,
    /// and returns its height
    fn check_subtree<'a>(
        nodes: &'a [TreeNode],
        idx: Option<usize>,
        names: &mut Vec<&'a str>,
    ) -> usize {
        let Some(node) = idx.map(|idx| &nodes[idx]) else {
            return 0;
        };
        let left = check_subtree(nodes, node.left, names);
        names.push(node.entry.get_name());
        let right = check_subtree(nodes, node.right, names);
        assert!(
            left.abs_diff(right) <= 1,
            "{} is unbalanced",
            node.entry.get_name()
        );
        1 + left.max(right)
    }

    #[test]
    fn test_balanced_tree_shape() {
        let mut builder = DirectoryEntryTableBuilder::default();
        for name in ["E", "A", "G", "C", "B", "F", "D"] {
            builder.add_file(name, 10).unwrap();
        }
        let writer = builder.build().unwrap();

        let name = |idx: Option<usize>| idx.map(|idx| writer.nodes[idx].entry.get_name());
        let preorder: Vec<_> = writer
            .nodes
            .iter()
            .map(|node| (node.entry.get_name(), name(node.left), name(node.right)))
            .collect();
        assert_eq!(
            preorder,
            [
                ("D", Some("B"), Some("F")),
                ("B", Some("A"), Some("C")),
                ("A", None, None),
                ("C", None, None),
                ("F", Some("E"), Some("G")),
                ("E", None, None),
                ("G", None, None),
            ]
        );
    }

    #[test]
    fn test_tree_balanced_and_ordered() {
        let mut rng = StdRng::seed_from_u64(SHUFFLE_SEED);
        for len in 1..=200 {
            let mut sorted: Vec<_> = (0..len)
                .map(|i| format!("{}{i}", ["a", "B", "a_", "A-b"][i % 4]))
                .collect();
            sorted.sort_by(|a, b| cmp_ignore_case_utf8(a, b));
            let mut shuffled = sorted.clone();
            shuffled.shuffle(&mut rng);

            let mut builder = DirectoryEntryTableBuilder::default();
            for name in &shuffled {
                builder.add_file(name, 10).unwrap();
            }
            let writer = builder.build().unwrap();

            // The reader starts its search at the table's first entry
            let mut inorder = Vec::new();
            check_subtree(&writer.nodes, Some(0), &mut inorder);
            assert_eq!(inorder, sorted, "{len} entries");
        }
    }

    #[test]
    fn test_duplicate_names_rejected() {
        let mut builder = DirectoryEntryTableBuilder::new(ImagePath::from("/dir"));
        builder.add_file("a.txt", 10).unwrap();
        builder.add_file("b.txt", 10).unwrap();
        builder.add_dir("A.TXT", 20, 1234).unwrap();
        assert!(matches!(
            builder.build(),
            Err(Error::DuplicateFileName(path)) if path == ImagePath::from("/dir/A.TXT")
        ));
    }

    #[test]
    fn test_dirtab_writer_reject_long_name() {
        let long_name: String = core::iter::repeat_n('a', 260).collect();
        let mut writer = DirectoryEntryTableBuilder::default();
        assert!(matches!(
            writer.add_dir(&long_name, 20, 1234),
            Err(Error::FileNameTooLong(path)) if path == ImagePath::from(long_name.as_str())
        ));
    }

    #[test]
    fn test_dirtab_writer_name_len_is_encoded_len() {
        let name = "é".repeat(130);
        let mut writer = DirectoryEntryTableBuilder::default();
        writer.add_file(&name, 10).unwrap();

        assert_eq!(writer.build().unwrap().dirtab_size(), 0xe + 130);
    }

    #[test]
    fn test_dirtab_writer_reject_unsafe_name() {
        let mut writer = DirectoryEntryTableBuilder::new(ImagePath::from("/dir"));
        for name in ["a:b", "a\\b"] {
            assert!(
                matches!(
                    writer.add_file(name, 10),
                    Err(Error::InvalidFileName(path))
                        if path == ImagePath::from("/dir").join(name)
                ),
                "{name:?}"
            );
        }
    }

    #[test]
    fn test_dirtab_writer_unencodable_name_names_path() {
        let mut writer = DirectoryEntryTableBuilder::new(ImagePath::from("/dir"));
        assert!(matches!(
            writer.add_file("日本.txt", 10),
            Err(Error::FileNameNotEncodable(path)) if path == ImagePath::from("/dir/日本.txt")
        ));
    }
}

use crate::blockdev::BlockDeviceRead;
use crate::layout::{cmp_ignore_case_utf8, DirectoryEntryNode, DirectoryEntryTable};
use crate::read::TableRanges;
use crate::write::fs::ImagePath;
use crate::Error;
use std::collections::BTreeSet;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum DirentSearchResult {
    Found,
    NextOffset(u64),
}

/// Converts a child entry's dword offset to a byte offset in the table.
fn child_offset(dword_offset: u16) -> Option<u64> {
    match dword_offset {
        0 | 0xffff => None,
        offset => Some(4 * u64::from(offset)),
    }
}

impl DirectoryEntryTable {
    fn dirent_search_next_offset(
        &self,
        dirent: &DirectoryEntryNode,
        name: &str,
    ) -> crate::Result<DirentSearchResult> {
        let dirent_name = dirent.name_str()?;
        log::debug!("[find_dirent] Node name {}", dirent_name);

        let cmp = cmp_ignore_case_utf8(name, &dirent_name);
        log::debug!("[find_dirent] Comparison result: {:?}", cmp);

        let child = match cmp {
            core::cmp::Ordering::Equal => return Ok(DirentSearchResult::Found),
            core::cmp::Ordering::Less => dirent.node.left_entry_offset,
            core::cmp::Ordering::Greater => dirent.node.right_entry_offset,
        };

        let child = child_offset(child).ok_or(Error::DoesNotExist)?;
        Ok(DirentSearchResult::NextOffset(self.offset(child)?))
    }

    fn find_dirent<BDR: BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
        name: &str,
    ) -> crate::Result<DirectoryEntryNode> {
        log::debug!("[find_dirent] Called on {}", name);
        if self.is_empty() {
            return Err(Error::DirectoryEmpty);
        }

        let mut visited = BTreeSet::new();
        let mut offset = self.offset(0)?;
        loop {
            if !visited.insert(offset) {
                break Err(Error::Loop);
            }

            let dirent = DirectoryEntryNode::read_from_disk(dev, offset)?;
            let dirent = dirent.ok_or(Error::DoesNotExist)?;
            log::trace!("[find_dirent] Found node: {:?}", dirent.node);

            match self.dirent_search_next_offset(&dirent, name)? {
                DirentSearchResult::Found => break Ok(dirent),
                DirentSearchResult::NextOffset(next_offset) => offset = next_offset,
            }
        }
    }

    /// Retrieves the directory entry node corresponding to the provided path,
    /// if it exists.
    ///
    /// Returns None if the root path is provided (root has no dirent)
    /// or the path does not exist.
    pub fn walk_path<BDR: BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
        path: &ImagePath,
    ) -> crate::Result<DirectoryEntryNode> {
        log::debug!("[walk_path] Called on {}", path);
        let mut dirent_tab = *self;
        let mut dir = ImagePath::root();
        let mut path_iter = path.components().peekable();

        while let Some(segment) = path_iter.next() {
            let dirent = dirent_tab
                .find_dirent(dev, segment)
                .map_err(|err| err.in_directory(dir.clone()))?;
            log::debug!("[walk_path] Found dirent: {:?}", dirent.name_str());
            log::trace!("[walk_path] Node: {:?}", dirent.node);
            let dirent_data = &dirent.node.dirent;

            if path_iter.peek().is_none() {
                return Ok(dirent);
            }

            dirent_tab = dirent_data.dirent_table().ok_or(Error::IsNotDirectory)?;
            dir.push(segment);
        }

        Err(Error::NoDirent)
    }

    /// Walks the directory entry table in preorder, returning all directory entries.
    pub fn walk_dirent_tree<BDR: BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
    ) -> crate::Result<Vec<DirectoryEntryNode>> {
        log::debug!("[walk_dirent_tree] {:?}", self);

        let mut dirents = vec![];
        if self.is_empty() {
            return Ok(dirents);
        }

        let mut visited = BTreeSet::new();
        let mut stack = vec![0];
        while let Some(top) = stack.pop() {
            if !visited.insert(top) {
                return Err(Error::Loop);
            }

            let offset = self.offset(top)?;
            let dirent = DirectoryEntryNode::read_from_disk(dev, offset)?;

            if let Some(dirent) = dirent {
                log::debug!(
                    "[walk_dirent_tree] Found dirent \"{}\"",
                    dirent.name_str().unwrap_or("<invalid dirent name>".into())
                );
                log::trace!("[walk_dirent_tree] Node: {:?} at offset {}", dirent, top);

                stack.extend(child_offset(dirent.node.right_entry_offset));
                stack.extend(child_offset(dirent.node.left_entry_offset));

                dirents.push(dirent);
            }
        }

        Ok(dirents)
    }

    pub fn file_tree<BDR: BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
    ) -> crate::Result<Vec<(ImagePath, DirectoryEntryNode)>> {
        let mut dirents = vec![];

        // Only tables with entries count: empty ones may share an all-padding sector
        let mut tables_read = TableRanges::default();
        let mut stack = vec![(ImagePath::root(), *self)];
        while let Some((parent, tree)) = stack.pop() {
            log::debug!("[file_tree] Descending through {}", parent);
            let children = tree
                .walk_dirent_tree(dev)
                .map_err(|err| err.in_directory(parent.clone()))?;
            if !children.is_empty() && tables_read.overlapping_or_insert(&tree, || ()).is_some() {
                return Err(Error::Loop);
            }

            for child in children {
                if let Some(dirent_table) = child.node.dirent.dirent_table() {
                    let child_name = child.name_str()?;
                    stack.push((parent.join(&child_name), dirent_table));
                }

                dirents.push((parent.clone(), child));
            }
        }

        Ok(dirents)
    }
}

#[cfg(test)]
mod test {
    use crate::{
        layout::{
            DirectoryEntryDiskData, DirectoryEntryDiskNode, DirectoryEntryNode,
            DirectoryEntryTable, DirentAttributes, DiskRegion,
        },
        read::dirent_table::DirentSearchResult,
        Error,
    };

    #[rustfmt::skip]
    const SELF_LOOP: [u8; 32] = [
        // Offset 0x00 (0)
        0, 0, 4, 0, // Right child -> B
        1, 0, 0, 0,
        2, 0, 0, 0,
        0, 1, b'A', 0xff,
        // Offset 0x10 (4)
        0, 0, 4, 0, // Right child -> B itself
        3, 0, 0, 0,
        4, 0, 0, 0,
        0, 1, b'B', 0xff,
    ];

    pub fn name_bytes_from(name: &str) -> [u8; 256] {
        assert!(name.len() <= 256);
        let mut out = [0u8; 256];
        out[..name.len()].copy_from_slice(name.as_bytes());
        out
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_found_match() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 257,
                right_entry_offset: 514,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table
            .dirent_search_next_offset(&dirent, "Bc")
            .expect("Dirent search should succeed");
        assert_eq!(res, DirentSearchResult::Found);
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_left_child() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 257,
                right_entry_offset: 514,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table
            .dirent_search_next_offset(&dirent, "ba")
            .expect("Dirent search should succeed");
        assert_eq!(res, DirentSearchResult::NextOffset(4 * 257));
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_right_child() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 257,
                right_entry_offset: 514,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table
            .dirent_search_next_offset(&dirent, "bd")
            .expect("Dirent search should succeed");
        assert_eq!(res, DirentSearchResult::NextOffset(4 * 514));
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_zero_left_child_does_not_exist() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 0,
                right_entry_offset: 514,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table.dirent_search_next_offset(&dirent, "ba");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_zero_right_child_does_not_exist() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 257,
                right_entry_offset: 0,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table.dirent_search_next_offset(&dirent, "bd");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_ff_left_child_does_not_exist() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 0xffff,
                right_entry_offset: 514,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table.dirent_search_next_offset(&dirent, "ba");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_dirent_next_offset_ff_right_child_does_not_exist() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let dirent = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 257,
                right_entry_offset: 0xffff,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion { sector: 1, size: 2 },
                    attributes: DirentAttributes(255),
                    filename_length: 2,
                },
            },
            name: name_bytes_from("Bc"),
            offset: 0,
        };

        let res = table.dirent_search_next_offset(&dirent, "bd");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_find_dirent_empty_zero_size() {
        let table = DirectoryEntryTable {
            region: DiskRegion { sector: 0, size: 0 },
        };

        let mut dev = [];
        let res = table.find_dirent(dev.as_mut_slice(), "bd");
        assert!(matches!(res, Err(Error::DirectoryEmpty)));
    }

    #[test]
    fn test_read_dirtab_find_dirent_ff_filled_sector() {
        let mut dev = [0xffu8; 2048];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.find_dirent(dev.as_mut_slice(), "bd");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_find_dirent_empty_filled_sector() {
        let mut dev = [0u8; 2048];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.find_dirent(dev.as_mut_slice(), "bd");
        assert!(matches!(res, Err(Error::DoesNotExist)));
    }

    #[test]
    fn test_read_dirtab_find_dirent_tree_search_succeeds() {
        #[rustfmt::skip]
        let mut dev = [
            0, 0, 4, 0, // Right child -> BE
            1, 0, 0, 0,
            2, 0, 0, 0,
            0xff, 2, b'B', b'c',
            8, 0, 0, 0, // Left child -> Bd
            3, 0, 0, 0,
            4, 0, 0, 0,
            0xff, 2, b'B', b'E',
            0, 0, 0, 0,
            5, 0, 0, 0,
            6, 0, 0, 0,
            0xff, 2, b'B', b'd',
        ];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table
            .find_dirent(dev.as_mut_slice(), "bd")
            .expect("Search should succeed");
        assert_eq!(
            res,
            DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 0,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector: 5, size: 6 },
                        attributes: DirentAttributes(255),
                        filename_length: 2,
                    },
                },
                name: name_bytes_from("Bd"),
                offset: 0x20,
            }
        );
    }

    #[test]
    fn test_read_dirtab_walk_path_root() {
        let mut dev = [];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.walk_path(dev.as_mut_slice(), &"/".into());
        assert!(matches!(res, Err(Error::NoDirent)));
    }

    #[test]
    fn test_find_dirent_child_at_dword_offset_255() {
        #[rustfmt::skip]
        let a = [
            0, 0, 255, 0, // Right child -> B
            1, 0, 0, 0,
            2, 0, 0, 0,
            0, 1, b'A',
        ];
        #[rustfmt::skip]
        let b = [
            0, 0, 0, 0,
            3, 0, 0, 0,
            4, 0, 0, 0,
            0, 1, b'B',
        ];
        let mut dev = [0xffu8; 2048];
        dev[..a.len()].copy_from_slice(&a);
        dev[1020..][..b.len()].copy_from_slice(&b);
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table
            .walk_path(dev.as_mut_slice(), &"/B".into())
            .expect("Path walk should succeed");
        assert_eq!(res.offset, 1020);
    }

    #[test]
    fn test_find_dirent_loop() {
        let mut dev = SELF_LOOP;
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.walk_path(dev.as_mut_slice(), &"/C".into());
        assert!(matches!(res, Err(Error::Loop)));
    }

    #[test]
    fn test_walk_path_all_slashes_is_root() {
        let mut dev = [];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.walk_path(dev.as_mut_slice(), &"//".into());
        assert!(matches!(res, Err(Error::NoDirent)));
    }

    #[test]
    fn test_walk_dirent_tree_self_loop() {
        let mut dev = SELF_LOOP;
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.walk_dirent_tree(dev.as_mut_slice());
        assert!(matches!(res, Err(Error::Loop)));
    }

    #[test]
    fn test_file_tree_directory_loop() {
        let directory = DirentAttributes(0).with_directory(true).0;
        #[rustfmt::skip]
        let mut dev = [
            0, 0, 0, 0,
            0, 0, 0, 0, // Table at sector 0, the root's own
            0, 8, 0, 0,
            directory, 1, b'D', 0xff,
        ];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.file_tree(dev.as_mut_slice());
        assert!(matches!(res, Err(Error::Loop)));
    }

    #[test]
    fn test_file_tree_shared_table() {
        let directory = DirentAttributes(0).with_directory(true).0;
        #[rustfmt::skip]
        let dirs_into = |sector: u8| [
            0, 0, 4, 0, // Right child -> b
            sector, 0, 0, 0,
            0, 8, 0, 0,
            directory, 1, b'a', 0xff,
            0, 0, 0, 0,
            sector, 0, 0, 0,
            0, 8, 0, 0,
            directory, 1, b'b', 0xff,
        ];
        #[rustfmt::skip]
        let file = [
            0, 0, 0, 0,
            3, 0, 0, 0,
            1, 0, 0, 0,
            0, 1, b'f', 0xff,
        ];
        let mut dev = [0xffu8; 3 * 2048];
        dev[..32].copy_from_slice(&dirs_into(1));
        dev[2048..][..32].copy_from_slice(&dirs_into(2));
        dev[2 * 2048..][..file.len()].copy_from_slice(&file);
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.file_tree(dev.as_mut_slice());
        assert!(matches!(res, Err(Error::Loop)));
    }

    #[test]
    fn test_file_tree_overlapping_tables() {
        // Tables at sectors 1 to 4, each running to the end of the image
        const SECTORS: u32 = 5;
        #[rustfmt::skip]
        let file = [
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 0, 0, 0,
            0, 1, b'f', 0xff,
        ];
        let mut dev = [0xffu8; SECTORS as usize * 2048];
        for (index, name) in (b'a'..=b'd').enumerate() {
            let sector = index as u32 + 1;
            let right_entry_offset = if name < b'd' { 4 * sector as u16 } else { 0 };
            let dir = DirectoryEntryDiskNode {
                left_entry_offset: 0,
                right_entry_offset,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion {
                        sector,
                        size: (SECTORS - sector) * 2048,
                    },
                    attributes: DirentAttributes(0).with_directory(true),
                    filename_length: 1,
                },
            };
            dev[16 * index..][..0xe].copy_from_slice(&dir.to_bytes());
            dev[16 * index + 0xe] = name;
            dev[sector as usize * 2048..][..file.len()].copy_from_slice(&file);
        }
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table.file_tree(dev.as_mut_slice());
        assert!(matches!(res, Err(Error::Loop)));
    }

    #[test]
    fn test_file_tree_empty_tables_share_sector() {
        let directory = DirentAttributes(0).with_directory(true).0;
        #[rustfmt::skip]
        let root = [
            0, 0, 4, 0, // Right child -> b
            1, 0, 0, 0, // Table at sector 1, all padding
            0, 8, 0, 0,
            directory, 1, b'a', 0xff,
            0, 0, 0, 0,
            1, 0, 0, 0,
            0, 8, 0, 0,
            directory, 1, b'b', 0xff,
        ];
        let mut dev = [0xffu8; 2 * 2048];
        dev[..root.len()].copy_from_slice(&root);
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table
            .file_tree(dev.as_mut_slice())
            .expect("File tree should succeed");
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn test_read_dirtab_walk_dirent_tree_empty_zero_size() {
        let table = DirectoryEntryTable {
            region: DiskRegion { sector: 0, size: 0 },
        };

        let mut dev = [];
        let res = table
            .walk_dirent_tree(dev.as_mut_slice())
            .expect("Dirtree walk should succeed");
        assert_eq!(&res, &[]);
    }

    #[test]
    fn test_read_dirtab_walk_dirent_tree_empty_zero_filled() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let mut dev = [0u8; 2048];
        let res = table
            .walk_dirent_tree(dev.as_mut_slice())
            .expect("Dirtree walk should succeed");
        assert_eq!(&res, &[]);
    }

    #[test]
    fn test_read_dirtab_walk_dirent_tree_returns_entries_in_preorder() {
        #[rustfmt::skip]
        let mut dev = [
            // Offset 0x00 (0)
            12, 0, 4, 0, // Left child -> Aa, Right child -> Bf
            1, 0, 0, 0,
            2, 0, 0, 0,
            0xff, 2, b'B', b'c',
            // Offset 0x10 (4)
            8, 0, 0, 0, // Left child -> Bd
            3, 0, 0, 0,
            4, 0, 0, 0,
            0xff, 2, b'B', b'f',
            // Offset 0x20 (8)
            0, 0, 16, 0, // Right child -> Be
            5, 0, 0, 0,
            6, 0, 0, 0,
            0xff, 2, b'B', b'd',
            // Offset 0x30 (12)
            0, 0, 0, 0,
            7, 0, 0, 0,
            8, 0, 0, 0,
            0xff, 2, b'A', b'a',
            // Offset 0x40 (16)
            0xff, 0xff, 0xff, 0xff,
            9, 0, 0, 0,
            10, 0, 0, 0,
            0xff, 2, b'B', b'e',
        ];
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let res = table
            .walk_dirent_tree(dev.as_mut_slice())
            .expect("Dirtree walk should succeed");

        fn dirent_node(
            name: &str,
            offset: u64,
            lchild: u16,
            rchild: u16,
            sector: u32,
            size: u32,
        ) -> DirectoryEntryNode {
            DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: lchild,
                    right_entry_offset: rchild,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector, size },
                        attributes: DirentAttributes(0xff),
                        filename_length: name.len() as u8,
                    },
                },
                name: name_bytes_from(name),
                offset,
            }
        }
        assert_eq!(res.len(), 5);
        assert_eq!(res[0], dirent_node("Bc", 0x00, 12, 4, 1, 2));
        assert_eq!(res[1], dirent_node("Aa", 0x30, 0, 0, 7, 8));
        assert_eq!(res[2], dirent_node("Bf", 0x10, 8, 0, 3, 4));
        assert_eq!(res[3], dirent_node("Bd", 0x20, 0, 16, 5, 6));
        assert_eq!(res[4], dirent_node("Be", 0x40, 0xffff, 0xffff, 9, 10));
    }
}

#[cfg(test)]
mod test_with_write {
    use super::test::name_bytes_from;
    use crate::{
        blockdev::BlockDeviceWrite,
        layout::{
            DirectoryEntryDiskData, DirectoryEntryDiskNode, DirectoryEntryNode,
            DirectoryEntryTable, DirentAttributes, DiskRegion, SECTOR_SIZE_U64,
        },
        write::{
            fs::{
                ImagePath, MemoryFilesystem, SectorLinearBlockDevice, SectorLinearBlockFilesystem,
                SectorLinearImage,
            },
            img::NoOpProgressVisitor,
        },
    };

    #[test]
    fn test_read_dirtab_walk_path_multiple() {
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(MemoryFilesystem::default());

        let mut push_table = |sector: u8, first_char: char| {
            #[rustfmt::skip]
            let table_bytes = [
                0, 0, 4, 0, // Right child -> BE
                sector + 1, 0, 0, 0,
                0, 8, 0, 0,
                0xff, 2, first_char as u8, b'c',
                8, 0, 0, 0, // Left child -> Bd
                sector + 1, 0, 0, 0,
                0, 8, 0, 0,
                0xff, 2, first_char as u8, b'E',
                0, 0, 0, 0,
                sector + 1, 0, 0, 0,
                0, 8, 0, 0,
                0xff, 2, first_char as u8, b'd',
            ];

            slbd.write(sector as u64 * SECTOR_SIZE_U64, &table_bytes)
                .expect("Write should succeed");
        };

        push_table(33, 'B');
        push_table(34, 'C');
        push_table(35, 'A');

        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 33,
                size: 2048,
            },
        };

        let mut dev = SectorLinearImage::new(&slbd, &mut slbfs);

        let res = table
            .walk_path(&mut dev, &"/Bd/CE/Ac".into())
            .expect("Path walk should succeed");
        assert_eq!(
            res,
            DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 4,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion {
                            sector: 36,
                            size: 2048,
                        },
                        attributes: DirentAttributes(255),
                        filename_length: 2,
                    },
                },
                name: name_bytes_from("Ac"),
                offset: 35 * SECTOR_SIZE_U64,
            }
        );
    }

    #[test]
    fn test_read_dirtab_file_tree() {
        let mut fs = MemoryFilesystem::default();
        fs.touch("/a/b/c");
        fs.touch("/a/b/d");
        fs.touch("/a/e");
        fs.touch("/f");

        let mut slbd = SectorLinearBlockDevice::default();
        let mut fs = SectorLinearBlockFilesystem::new(fs);
        crate::write::img::create_xdvdfs_image(&mut fs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");
        let mut dev = SectorLinearImage::new(&slbd, &mut fs);

        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 33,
                size: 2048,
            },
        };
        let tree = table
            .file_tree(&mut dev)
            .expect("File tree should not fail");
        assert_eq!(tree.len(), 6);

        assert_eq!(tree[0].0, ImagePath::root());
        assert_eq!(tree[0].1.name, name_bytes_from("a"));
        assert_eq!(tree[1].0, ImagePath::root());
        assert_eq!(tree[1].1.name, name_bytes_from("f"));
        assert_eq!(tree[2].0, ImagePath::from("/a"));
        assert_eq!(tree[2].1.name, name_bytes_from("b"));
        assert_eq!(tree[3].0, ImagePath::from("/a"));
        assert_eq!(tree[3].1.name, name_bytes_from("e"));
        assert_eq!(tree[4].0, ImagePath::from("/a/b"));
        assert_eq!(tree[4].1.name, name_bytes_from("c"));
        assert_eq!(tree[5].0, ImagePath::from("/a/b"));
        assert_eq!(tree[5].1.name, name_bytes_from("d"));
    }
}

use encoding_rs::WINDOWS_1252;
use std::borrow::Cow;

use super::{is_safe_name, DirectoryEntryDiskNode};
use crate::Error;

/// In-memory structure to contain an on-disk tree node and
/// file name information.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct DirectoryEntryNode {
    pub node: DirectoryEntryDiskNode,
    pub name: [u8; 256],
    pub offset: u64,
}

impl DirectoryEntryNode {
    /// Deserializes a DirectoryEntryNode from a buffer
    /// containing a DirectoryEntryDiskNode. This does not
    /// populate the dirent name. Returns None if the node
    /// is empty.
    pub fn deserialize(
        dirent_buf: &[u8; 0xe],
        offset: u64,
    ) -> Result<Option<Self>, bincode::Error> {
        // Empty directory entries are filled with 0xff or 0x00
        if dirent_buf == &[0xff; 0xe] || dirent_buf == &[0x00; 0xe] {
            return Ok(None);
        }

        let node = DirectoryEntryDiskNode::deserialize(dirent_buf)?;
        Ok(Some(Self {
            node,
            name: [0; 256],
            offset,
        }))
    }

    pub fn name_slice(&self) -> &[u8] {
        let name_len = self.node.dirent.filename_length as usize;
        &self.name[0..name_len]
    }

    /// Returns the file name decoded to UTF-8, or an error if it can't be
    /// decoded or isn't safe to use as a single path component.
    pub fn name_str(&self) -> crate::Result<Cow<'_, str>> {
        let name = WINDOWS_1252
            .decode_without_bom_handling_and_without_replacement(self.name_slice())
            .ok_or(Error::InvalidNameEncoding)?;

        if !is_safe_name(&name) {
            return Err(Error::UnsafeName(name.into_owned()));
        }

        Ok(name)
    }
}

#[cfg(test)]
mod test {
    use crate::layout::{
        DirectoryEntryDiskData, DirectoryEntryDiskNode, DirentAttributes, DiskRegion,
    };

    use super::DirectoryEntryNode;
    use crate::Error;

    #[test]
    fn test_layout_dirent_node_deserialize_empty_ff_filled() {
        let dirent_buf = [0xff; 0xe];
        let dirent_node = DirectoryEntryNode::deserialize(&dirent_buf, 10);
        assert!(dirent_node.is_ok_and(|node| node.is_none()));
    }

    #[test]
    fn test_layout_dirent_node_deserialize_empty_zero_filled() {
        let dirent_buf = [0x00; 0xe];
        let dirent_node = DirectoryEntryNode::deserialize(&dirent_buf, 10);
        assert!(dirent_node.is_ok_and(|node| node.is_none()));
    }

    #[test]
    fn test_layout_dirent_node_deserialize_entry() {
        let mut dirent_buf = [0x00; 0xe];
        dirent_buf[0] = 1;
        dirent_buf[1] = 1;
        dirent_buf[2] = 2;
        dirent_buf[3] = 2;
        dirent_buf[4] = 3;
        dirent_buf[8] = 4;
        dirent_buf[12] = 0;
        dirent_buf[13] = 5;

        let dirent_node = DirectoryEntryNode::deserialize(&dirent_buf, 10)
            .expect("Node should deserialize correctly")
            .expect("Node should be present");
        assert_eq!({ dirent_node.node.left_entry_offset }, 257);
        assert_eq!({ dirent_node.node.right_entry_offset }, 514);
        assert_eq!({ dirent_node.node.dirent.data.sector }, 3);
        assert_eq!({ dirent_node.node.dirent.data.size }, 4);
        assert_eq!(dirent_node.node.dirent.attributes.attrs(), 0);
        assert_eq!(dirent_node.node.dirent.filename_length, 5);
        assert_eq!(dirent_node.offset, 10);
    }

    #[test]
    fn test_layout_dirent_node_name_decode_windows_1252() {
        let mut name = [0u8; 256];
        name[0] = b'A';
        name[1] = 159; // Ÿ

        let node = DirectoryEntryNode {
            node: DirectoryEntryDiskNode {
                left_entry_offset: 0,
                right_entry_offset: 0,
                dirent: DirectoryEntryDiskData {
                    data: DiskRegion {
                        size: 10,
                        sector: 0,
                    },
                    attributes: DirentAttributes(0),
                    filename_length: 2,
                },
            },
            name,
            offset: 0,
        };

        let decoded_name = node.name_str().expect("Name should decode");
        assert_eq!(decoded_name, "AŸ");
    }

    #[test]
    fn test_name_str_rejects_unsafe_names() {
        fn node_named(name: &[u8]) -> DirectoryEntryNode {
            let mut node = DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 0,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { size: 0, sector: 0 },
                        attributes: DirentAttributes(0),
                        filename_length: name.len() as u8,
                    },
                },
                name: [0; 256],
                offset: 0,
            };
            node.name[..name.len()].copy_from_slice(name);
            node
        }

        for name in ["", ".", "..", "../x", "a/b", "a\\b", "a\0b", "C:x"] {
            assert!(
                matches!(
                    node_named(name.as_bytes()).name_str(),
                    Err(Error::UnsafeName(_))
                ),
                "{name:?}"
            );
        }
        assert_eq!(node_named(b"...").name_str().unwrap(), "...");
        assert_eq!(node_named(b"caf\xe9").name_str().unwrap(), "café");
    }
}

use super::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};

/// On-disk representation of a directory entry tree node,
/// including the left and right children, and data.
///
/// Does not include the file name or padding.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct DirectoryEntryDiskNode {
    pub left_entry_offset: u16,
    pub right_entry_offset: u16,
    pub dirent: DirectoryEntryDiskData,
}

impl DirectoryEntryDiskNode {
    pub fn from_bytes(buf: &[u8; 0xe]) -> Self {
        Self {
            left_entry_offset: u16::from_le_bytes(buf[0..2].try_into().unwrap()),
            right_entry_offset: u16::from_le_bytes(buf[2..4].try_into().unwrap()),
            dirent: DirectoryEntryDiskData {
                data: DiskRegion::from_bytes(buf[4..12].try_into().unwrap()),
                attributes: DirentAttributes(buf[12]),
                filename_length: buf[13],
            },
        }
    }

    pub fn to_bytes(&self) -> [u8; 0xe] {
        let mut buf = [0; 0xe];
        buf[0..2].copy_from_slice(&self.left_entry_offset.to_le_bytes());
        buf[2..4].copy_from_slice(&self.right_entry_offset.to_le_bytes());
        buf[4..12].copy_from_slice(&self.dirent.data.to_bytes());
        buf[12] = self.dirent.attributes.0;
        buf[13] = self.dirent.filename_length;
        buf
    }
}

#[cfg(test)]
mod test {
    use crate::layout::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};

    use super::DirectoryEntryDiskNode;

    #[test]
    fn test_layout_dirent_disk_node_bytes_roundtrip() {
        let bytes: [u8; 0xe] = [1, 1, 2, 2, 1, 0, 0, 0, 2, 0, 0, 0, 255, 7];
        let node = DirectoryEntryDiskNode {
            left_entry_offset: 257,
            right_entry_offset: 514,
            dirent: DirectoryEntryDiskData {
                data: DiskRegion { sector: 1, size: 2 },
                attributes: DirentAttributes(255),
                filename_length: 7,
            },
        };

        assert_eq!(DirectoryEntryDiskNode::from_bytes(&bytes), node);
        assert_eq!(node.to_bytes(), bytes);
    }
}

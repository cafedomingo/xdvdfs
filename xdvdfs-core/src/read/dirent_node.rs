use crate::{blockdev::BlockDeviceRead, layout::DirectoryEntryNode};

impl DirectoryEntryNode {
    /// Read a DirectoryEntryNode from disk, including
    /// the name. Returns None if the node is not present.
    pub fn read_from_disk<BDR: BlockDeviceRead + ?Sized>(
        dev: &mut BDR,
        offset: u64,
    ) -> crate::Result<Option<Self>> {
        let mut dirent_buf = [0; 0xe];
        dev.read(offset, &mut dirent_buf)?;

        let Some(mut dirent) = Self::from_bytes(&dirent_buf, offset) else {
            return Ok(None);
        };

        let name_len = dirent.node.dirent.filename_length as usize;
        let name_buf = &mut dirent.name[0..name_len];
        dev.read(offset + 0xe, name_buf)?;

        Ok(Some(dirent))
    }
}

#[cfg(test)]
mod test {
    use crate::layout::{
        DirectoryEntryDiskData, DirectoryEntryDiskNode, DirectoryEntryNode, DirentAttributes,
        DiskRegion,
    };

    #[test]
    fn test_read_dirent_node_from_disk_empty_ff_filled() {
        let mut data = [0xffu8; 0x10];
        let res = DirectoryEntryNode::read_from_disk(data.as_mut_slice(), 2);
        assert!(matches!(res, Ok(None)));
    }

    #[test]
    fn test_read_dirent_node_from_disk_empty_zero_filled() {
        let mut data = [0x00u8; 0x10];
        let res = DirectoryEntryNode::read_from_disk(data.as_mut_slice(), 2);
        assert!(matches!(res, Ok(None)));
    }

    #[test]
    fn test_read_dirent_node_from_disk_valid_dirent() {
        let data: &mut [u8] = &mut [
            0x2e, 0x2e, 0x2e, 0x2e, // padding for offset
            0x1, 0x1, 0x2, 0x2, 0x1, 0x0, 0x0, 0x0, 0x2, 0x0, 0x0, 0x0, 0xff, 0x2, b'A', b'b',
        ];

        let res =
            DirectoryEntryNode::read_from_disk(data, 4).expect("Read/deserialize should not fail");
        let mut dirent_name = [0u8; 256];
        dirent_name[0] = b'A';
        dirent_name[1] = b'b';

        assert_eq!(
            res,
            Some(DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 257,
                    right_entry_offset: 514,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector: 1, size: 2 },
                        attributes: DirentAttributes(255),
                        filename_length: 2,
                    },
                },
                name: dirent_name,
                offset: 4,
            })
        );
    }
}

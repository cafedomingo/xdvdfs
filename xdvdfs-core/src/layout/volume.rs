use super::{DirectoryEntryTable, DiskRegion, SECTOR_SIZE_USZ};

pub const VOLUME_HEADER_MAGIC: [u8; 0x14] = *b"MICROSOFT*XBOX*MEDIA";

/// XDVDFS volume information, located at sector 32 on the disk
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct VolumeDescriptor {
    magic0: [u8; 0x14],
    pub root_table: DirectoryEntryTable,
    pub filetime: u64,
    magic1: [u8; 0x14],
}

impl VolumeDescriptor {
    pub fn new(root_table: DirectoryEntryTable) -> Self {
        Self {
            magic0: VOLUME_HEADER_MAGIC,
            root_table,
            filetime: 0,
            magic1: VOLUME_HEADER_MAGIC,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic0 == VOLUME_HEADER_MAGIC && self.magic1 == VOLUME_HEADER_MAGIC
    }

    pub fn from_bytes(buf: &[u8; SECTOR_SIZE_USZ]) -> Self {
        Self {
            magic0: buf[0..0x14].try_into().unwrap(),
            root_table: DirectoryEntryTable {
                region: DiskRegion::from_bytes(buf[0x14..0x1c].try_into().unwrap()),
            },
            filetime: u64::from_le_bytes(buf[0x1c..0x24].try_into().unwrap()),
            magic1: buf[0x7ec..0x800].try_into().unwrap(),
        }
    }

    pub fn to_bytes(&self) -> [u8; SECTOR_SIZE_USZ] {
        let mut buf = [0; SECTOR_SIZE_USZ];
        buf[0..0x14].copy_from_slice(&self.magic0);
        buf[0x14..0x1c].copy_from_slice(&self.root_table.region.to_bytes());
        buf[0x1c..0x24].copy_from_slice(&self.filetime.to_le_bytes());
        buf[0x7ec..0x800].copy_from_slice(&self.magic1);
        buf
    }
}

#[cfg(test)]
mod test {
    use crate::layout::{DirectoryEntryTable, DiskRegion, VOLUME_HEADER_MAGIC};

    use super::VolumeDescriptor;

    #[test]
    fn test_layout_volume_create() {
        let root_table = DirectoryEntryTable {
            region: DiskRegion { size: 0, sector: 0 },
        };

        let volume = VolumeDescriptor::new(root_table);
        assert!(volume.is_valid());
    }

    #[test]
    fn test_layout_volume_invalid_magic0() {
        let root_table = DirectoryEntryTable {
            region: DiskRegion { size: 0, sector: 0 },
        };

        let mut volume = VolumeDescriptor::new(root_table);
        volume.magic0[0] = 1;
        assert!(!volume.is_valid());
    }

    #[test]
    fn test_layout_volume_invalid_magic1() {
        let root_table = DirectoryEntryTable {
            region: DiskRegion { size: 0, sector: 0 },
        };

        let mut volume = VolumeDescriptor::new(root_table);
        volume.magic1[0] = 1;
        assert!(!volume.is_valid());
    }

    #[test]
    fn test_layout_volume_bytes_roundtrip() {
        let mut bytes = [0u8; 0x800];
        bytes[0..0x14].copy_from_slice(&VOLUME_HEADER_MAGIC);
        bytes[0x7ec..0x800].copy_from_slice(&VOLUME_HEADER_MAGIC);
        bytes[0x14] = 10;
        bytes[0x18] = 20;
        bytes[0x1c] = 30;

        let mut volume = VolumeDescriptor::new(DirectoryEntryTable {
            region: DiskRegion {
                sector: 10,
                size: 20,
            },
        });
        volume.filetime = 30;

        assert_eq!(VolumeDescriptor::from_bytes(&bytes), volume);
        assert_eq!(volume.to_bytes(), bytes);
    }
}

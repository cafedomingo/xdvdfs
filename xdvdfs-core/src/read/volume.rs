use std::io;

use crate::blockdev::BlockDeviceRead;
use crate::layout::{VolumeDescriptor, SECTOR_SIZE_U64, SECTOR_SIZE_USZ};
use crate::Error;

/// Read the XDVDFS volume descriptor from sector 32 of the drive.
/// Errors with `InvalidVolume` if the device is too short to hold one or it
/// is invalid.
pub fn read_volume<BDR: BlockDeviceRead + ?Sized>(
    dev: &mut BDR,
) -> crate::Result<VolumeDescriptor> {
    let mut buffer = [0; SECTOR_SIZE_USZ];
    dev.read(32 * SECTOR_SIZE_U64, &mut buffer)
        .map_err(|err| match err.kind() {
            io::ErrorKind::UnexpectedEof => Error::InvalidVolume,
            _ => err.into(),
        })?;

    let volume = VolumeDescriptor::from_bytes(&buffer);
    if !volume.is_valid() {
        return Err(Error::InvalidVolume);
    }
    Ok(volume)
}

#[cfg(test)]
mod test {
    use std::io;

    use crate::{
        blockdev::{BlockDeviceRead, BlockDeviceWrite, OffsetWrapper},
        layout::{
            DirectoryEntryTable, DiskRegion, VolumeDescriptor, SECTOR_SIZE_U64, VOLUME_HEADER_MAGIC,
        },
        read::read_volume,
        write::fs::{
            MemoryFilesystem, SectorLinearBlockDevice, SectorLinearBlockFilesystem,
            SectorLinearImage,
        },
        Error,
    };

    #[test]
    fn test_read_volume_from_disk_not_enough_disk_space() {
        let mut data = [0u8];
        let res = read_volume(data.as_mut_slice());
        assert!(matches!(res, Err(Error::InvalidVolume)));
    }

    #[test]
    fn test_read_volume_propagates_io_errors() {
        struct FailingDevice;

        impl BlockDeviceRead for FailingDevice {
            fn read(&mut self, _offset: u64, _buffer: &mut [u8]) -> io::Result<()> {
                Err(io::Error::other("injected"))
            }
        }

        let res = read_volume(&mut FailingDevice);
        assert!(matches!(res, Err(Error::Io(_))), "{res:?}");
        let res = OffsetWrapper::new(FailingDevice).map(|_| ());
        assert!(matches!(res, Err(Error::Io(_))), "{res:?}");
    }

    #[test]
    fn test_read_volume_from_disk_invalid_volume() {
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(MemoryFilesystem::default());

        let mut volume = [0u8; 0x800];
        volume[0..0x14].copy_from_slice(&VOLUME_HEADER_MAGIC);
        volume[0x7ec..0x800].copy_from_slice(&VOLUME_HEADER_MAGIC);
        volume[0x14] = 10;
        volume[0x18] = 20;

        // Invalidate header magic number
        volume[0] = 0;
        volume[0x7ec] = 0;

        slbd.write(32 * SECTOR_SIZE_U64, &volume)
            .expect("Write should succeed");

        let mut dev = SectorLinearImage::new(&slbd, &mut slbfs);
        let res = read_volume(&mut dev);
        assert!(matches!(res, Err(Error::InvalidVolume)));
    }

    #[test]
    fn test_read_volume_from_disk_valid_volume() {
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(MemoryFilesystem::default());

        let mut volume = [0u8; 0x800];
        volume[0..0x14].copy_from_slice(&VOLUME_HEADER_MAGIC);
        volume[0x7ec..0x800].copy_from_slice(&VOLUME_HEADER_MAGIC);
        volume[0x14] = 10;
        volume[0x18] = 20;

        slbd.write(32 * SECTOR_SIZE_U64, &volume)
            .expect("Write should succeed");

        let mut dev = SectorLinearImage::new(&slbd, &mut slbfs);
        let res = read_volume(&mut dev).expect("Volume should be valid");
        let volume_expected = VolumeDescriptor::new(DirectoryEntryTable {
            region: DiskRegion {
                sector: 10,
                size: 20,
            },
        });
        assert_eq!(res, volume_expected);
    }
}

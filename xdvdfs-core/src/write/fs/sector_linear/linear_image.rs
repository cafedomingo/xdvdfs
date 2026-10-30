use core::ops::{Deref, DerefMut};
use std::io;

use crate::blockdev::BlockDeviceRead;
use crate::layout;
use crate::write::fs::{FilesystemCopier, SectorLinearBlockDevice, SectorLinearBlockFilesystem};
use crate::Error;

use super::SectorLinearBlockRegion;

pub struct SectorLinearImage<SLBD, SLBFS> {
    linear: SLBD,
    fs: SLBFS,
}

impl<F, SLBD, SLBFS> SectorLinearImage<SLBD, SLBFS>
where
    SLBD: Deref<Target = SectorLinearBlockDevice>,
    SLBFS: DerefMut<Target = SectorLinearBlockFilesystem<F>>,
{
    pub fn new(bdev: SLBD, fs: SLBFS) -> Self {
        Self { linear: bdev, fs }
    }
}

impl<F, SLBD, SLBFS> SectorLinearImage<SLBD, SLBFS>
where
    F: FilesystemCopier<[u8]>,
    SLBD: Deref<Target = SectorLinearBlockDevice>,
    SLBFS: DerefMut<Target = SectorLinearBlockFilesystem<F>>,
{
    pub fn read_linear(&mut self, offset: u64, size: u64) -> crate::Result<Vec<u8>> {
        let mut sector = offset / (layout::SECTOR_SIZE as u64);
        let mut position = offset % (layout::SECTOR_SIZE as u64);

        let size = size as usize;
        let mut buffer = vec![0; size];
        let mut index: usize = 0;

        let mut iter = self.linear.sector_range(sector..);

        while index < size {
            let Some((incoming_sector, contents)) = iter.next() else {
                // Out of sectors, truncate buffer to actual size
                buffer.resize(index, 0);
                break;
            };

            if incoming_sector > sector {
                let sector_gap = incoming_sector - sector;
                let empty_len = core::cmp::min(
                    (size - index) as u64,
                    sector_gap * layout::SECTOR_SIZE as u64 - position,
                );
                index += empty_len as usize;
                position = 0;
                sector += sector_gap;

                if index >= size {
                    break;
                }
            }

            let remaining = size - index;
            let content_offset = sector.saturating_sub(incoming_sector);
            let content_offset_sector = content_offset * layout::SECTOR_SIZE as u64;
            let to_read = core::cmp::min(
                remaining as u64,
                contents.size_bytes() - content_offset_sector - position,
            ) as usize;

            match contents {
                SectorLinearBlockRegion::RawData(data) => {
                    let data = &data[(content_offset_sector as usize)..];
                    let position = position as usize;
                    let end = position + to_read;
                    buffer[index..(index + to_read)].clone_from_slice(&data[position..end]);
                }
                SectorLinearBlockRegion::File { path, len } => {
                    let file_offset = content_offset_sector + position;
                    let file_read =
                        core::cmp::min(to_read, len.saturating_sub(file_offset) as usize);
                    self.fs.fs.copy_file_in(
                        path,
                        &mut buffer[index..(index + file_read)],
                        file_offset,
                        0,
                        file_read as u64,
                    )?;
                }
            }

            index += to_read;
            position = 0;
            sector += contents.size_sectors() - content_offset;
        }

        Ok(buffer)
    }
}

impl<F, SLBD, SLBFS> ciso::write::SectorReader for SectorLinearImage<SLBD, SLBFS>
where
    F: FilesystemCopier<[u8]>,
    SLBD: Deref<Target = SectorLinearBlockDevice> + Send + Sync,
    SLBFS: DerefMut<Target = SectorLinearBlockFilesystem<F>> + Send + Sync,
{
    type ReadError = crate::Error;

    fn size(&mut self) -> Result<u64, Self::ReadError> {
        Ok(self.linear.size())
    }

    fn read_sector(&mut self, sector: usize, sector_size: u32) -> Result<Vec<u8>, Self::ReadError> {
        let offset = sector as u64 * sector_size as u64;
        let mut data = self.read_linear(offset, sector_size as u64)?;
        if data.len() < sector_size as usize {
            data.resize(sector_size as usize, 0);
        }

        Ok(data)
    }
}

impl<F, SLBD, SLBFS> BlockDeviceRead for SectorLinearImage<SLBD, SLBFS>
where
    F: FilesystemCopier<[u8]>,
    SLBD: Deref<Target = SectorLinearBlockDevice>,
    SLBFS: DerefMut<Target = SectorLinearBlockFilesystem<F>>,
{
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let len = <[u8]>::len(buffer);
        let mut data = self
            .read_linear(offset, len as u64)
            .map_err(|err| match err {
                Error::Io(err) => err,
                err => io::Error::other(err),
            })?;
        data.resize(len, 0);
        buffer.copy_from_slice(&data);
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::write::fs::{ImagePath, MemoryFilesystem, SectorLinearBlockRegion};

    use super::{SectorLinearBlockDevice, SectorLinearBlockFilesystem, SectorLinearImage};

    #[test]
    fn test_linear_image_read_empty_image() {
        let memfs = MemoryFilesystem::default();
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        // Insert sector to give image size
        let data = vec![0; 2 * 2048];
        slbd.contents
            .insert(5, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 2048).expect("Read should return data");
        assert_eq!(data.len(), 2048);
        assert_eq!(data, vec![0; 2048]);
    }

    #[test]
    fn test_linear_image_eof_truncated_output() {
        let memfs = MemoryFilesystem::default();
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        // Insert sector to give image size
        let data = vec![0; 2048];
        slbd.contents
            .insert(0, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 4096).expect("Read should return data");
        assert_eq!(data.len(), 2048);
        assert_eq!(data, vec![0; 2048]);
    }

    #[test]
    fn test_linear_image_read_data() {
        let memfs = MemoryFilesystem::default();
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let data = vec![10; 2048];
        slbd.contents
            .insert(0, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 2048).expect("Read should return data");
        assert_eq!(data.len(), 2048);
        assert_eq!(data, vec![10; 2048]);
    }

    #[test]
    fn test_linear_image_read_data_offset() {
        let memfs = MemoryFilesystem::default();
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let data = vec![10; 2048];
        slbd.contents
            .insert(0, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));
        let data = vec![0; 2048];
        slbd.contents
            .insert(1, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image
            .read_linear(1024, 2048)
            .expect("Read should return data");
        assert_eq!(data.len(), 2048);
        assert_eq!(data[0..1024], vec![10; 1024]);
        assert_eq!(data[1024..], vec![0; 1024]);
    }

    #[test]
    fn test_linear_image_read_data_sized() {
        let memfs = MemoryFilesystem::default();
        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let data = vec![10; 2048];
        slbd.contents
            .insert(0, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 1024).expect("Read should return data");
        assert_eq!(data.len(), 1024);
        assert_eq!(data[0..1024], vec![10; 1024]);
    }

    #[test]
    fn test_linear_image_read_file() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &vec![10; 4000]);

        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let path = ImagePath::from("/a/b");
        slbd.contents
            .insert(0, SectorLinearBlockRegion::File { path, len: 4000 });

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 4096).expect("Read should return data");
        assert_eq!(data.len(), 4096);
        assert_eq!(data[0..4000], vec![10; 4000]);
        assert_eq!(data[4000..], vec![0; 96]);
    }

    #[test]
    fn test_linear_image_read_file_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &vec![10; 4000]);

        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let path = ImagePath::from("/a/b");
        slbd.contents
            .insert(0, SectorLinearBlockRegion::File { path, len: 4000 });

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image
            .read_linear(1024, 4096)
            .expect("Read should return data");
        assert_eq!(data.len(), 3072);

        // File size of 4000 - offset of 1024
        assert_eq!(data[0..2976], vec![10; 2976]);
        assert_eq!(data[2976..], vec![0; 96]);
    }

    #[test]
    fn test_linear_image_read_file_sized() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &vec![10; 4000]);

        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        let path = ImagePath::from("/a/b");
        slbd.contents
            .insert(0, SectorLinearBlockRegion::File { path, len: 4000 });

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        let data = image.read_linear(0, 3072).expect("Read should return data");
        assert_eq!(data.len(), 3072);
        assert_eq!(data[0..3072], vec![10; 3072]);
    }

    #[test]
    fn test_linear_image_read_sparse_multi_sector() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &vec![10; 2000]);

        let mut slbd = SectorLinearBlockDevice::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);

        slbd.contents.insert(
            3,
            SectorLinearBlockRegion::File {
                path: "/a/b".into(),
                len: 2000,
            },
        );
        let data = vec![15; 2048];
        slbd.contents
            .insert(5, SectorLinearBlockRegion::RawData(data.into_boxed_slice()));

        let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
        // Size: 1024 from sector 1, then sectors 2, 3, 4, 5
        let data = image
            .read_linear(3072, 2048 * 4 + 1024)
            .expect("Read should return data");
        assert_eq!(data.len(), 2048 * 4 + 1024);

        // Sector 1 and 2 are empty
        assert_eq!(data[0..3072], vec![0; 3072]);
        // Sector 3 is a file, last 48 bytes are zero
        assert_eq!(data[3072..5072], vec![10; 2000]);
        assert_eq!(data[5072..5120], vec![0; 48]);
        // Sector 4 is empty
        assert_eq!(data[5120..7168], vec![0; 2048]);
        // Sector 5 is data
        assert_eq!(data[7168..], vec![15; 2048]);
    }
}

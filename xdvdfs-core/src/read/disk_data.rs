use thiserror::Error;

use crate::layout::{DirectoryEntryDiskData, OutOfBounds};

#[non_exhaustive]
#[derive(Error, Debug, Copy, Clone, Eq, PartialEq)]
pub enum DiskDataReadError<E> {
    #[error("io error")]
    IOError(#[source] E),
    #[error("offset out of bounds")]
    SizeOutOfBounds(#[from] OutOfBounds),
}

/// Chunks this large keep reads from being syscall-bound, which they are
/// with 8 KiB chunks.
pub const DATA_CHUNK_LEN: usize = 1 << 20;

impl DirectoryEntryDiskData {
    pub fn read_data<BDR: crate::blockdev::BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
        buf: &mut [u8],
    ) -> Result<(), DiskDataReadError<BDR::ReadError>> {
        if self.data.size == 0 {
            return Ok(());
        }

        let offset = self.data.offset(0)?;
        dev.read(offset, buf).map_err(DiskDataReadError::IOError)?;
        Ok(())
    }

    /// Reads the data front to back and passes it to `consume` in chunks of at
    /// most `DATA_CHUNK_LEN` bytes, so memory use doesn't depend on the size
    /// recorded in the image.
    pub fn read_data_chunks<BDR: crate::blockdev::BlockDeviceRead + ?Sized>(
        &self,
        dev: &mut BDR,
        mut consume: impl FnMut(&[u8]),
    ) -> Result<(), DiskDataReadError<BDR::ReadError>> {
        let size = u64::from(self.data.size);
        let mut chunk = vec![0; size.min(DATA_CHUNK_LEN as u64) as usize];
        let mut done = 0;
        while done < size {
            let len = (size - done).min(chunk.len() as u64) as usize;
            let offset = self.data.offset(done)?;
            dev.read(offset, &mut chunk[..len])
                .map_err(DiskDataReadError::IOError)?;
            consume(&chunk[..len]);
            done += len as u64;
        }
        Ok(())
    }

    pub fn seek_to(
        &self,
        seek: &mut impl std::io::Seek,
    ) -> Result<u64, DiskDataReadError<std::io::Error>> {
        use std::io::SeekFrom;

        let offset = self.data.offset(0)?;
        let offset = seek
            .seek(SeekFrom::Start(offset))
            .map_err(DiskDataReadError::IOError)?;
        Ok(offset)
    }
}

#[cfg(test)]
mod test {
    use super::{DiskDataReadError, DATA_CHUNK_LEN};
    use crate::blockdev::ByteSliceOutOfBounds;
    use crate::layout::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};

    #[test]
    fn test_layout_dirent_disk_data_read_data_empty() {
        let mut data: [u8; 8] = [1; 8];

        let dirent = DirectoryEntryDiskData {
            data: DiskRegion { sector: 0, size: 0 },
            attributes: DirentAttributes(0),
            filename_length: 0,
        };

        let mut buf = [0; 8];
        dirent.read_data(data.as_mut_slice(), &mut buf).unwrap();

        assert_eq!(buf, [0; 8]);
    }

    #[test]
    fn test_layout_dirent_disk_data_read_data_non_empty() {
        let mut data: [u8; 10] = [1; 10];

        let dirent = DirectoryEntryDiskData {
            data: DiskRegion {
                sector: 0,
                size: 10,
            },
            attributes: DirentAttributes(0),
            filename_length: 0,
        };

        let mut buf = [0; 8];
        dirent.read_data(data.as_mut_slice(), &mut buf).unwrap();

        assert_eq!(buf, [1; 8]);
    }

    fn dirent(size: u32) -> DirectoryEntryDiskData {
        DirectoryEntryDiskData {
            data: DiskRegion { sector: 0, size },
            attributes: DirentAttributes(0),
            filename_length: 0,
        }
    }

    fn read_chunks(
        dirent: &DirectoryEntryDiskData,
        image: &mut [u8],
    ) -> Result<Vec<Vec<u8>>, DiskDataReadError<ByteSliceOutOfBounds>> {
        let mut chunks = Vec::new();
        dirent.read_data_chunks(image, |chunk| chunks.push(chunk.to_vec()))?;
        Ok(chunks)
    }

    #[test]
    fn test_layout_dirent_disk_data_read_data_chunks_empty() {
        let chunks = read_chunks(&dirent(0), &mut [1; 8]).unwrap();
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_layout_dirent_disk_data_read_data_chunks_small() {
        let chunks = read_chunks(&dirent(5), &mut [1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(chunks, [[1, 2, 3, 4, 5]]);
    }

    #[test]
    fn test_layout_dirent_disk_data_read_data_chunks_multiple() {
        let size = 2 * DATA_CHUNK_LEN + 13;
        let mut image: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

        let chunks = read_chunks(&dirent(size as u32), &mut image).unwrap();
        let lens: Vec<_> = chunks.iter().map(Vec::len).collect();
        assert_eq!(lens, [DATA_CHUNK_LEN, DATA_CHUNK_LEN, 13]);
        assert_eq!(chunks.concat(), image);
    }

    #[test]
    fn test_layout_dirent_disk_data_read_data_chunks_truncated_image() {
        let mut image = vec![1; DATA_CHUNK_LEN + 5];

        let mut consumed = 0;
        let res =
            dirent(2 * DATA_CHUNK_LEN as u32).read_data_chunks(image.as_mut_slice(), |chunk| {
                consumed += chunk.len();
            });
        assert!(res.is_err());
        assert_eq!(consumed, DATA_CHUNK_LEN);
    }
}

#[cfg(test)]
mod test_std {
    use crate::layout::{DirectoryEntryDiskData, DirentAttributes, DiskRegion};
    use std::io::Cursor;

    #[test]
    fn test_layout_dirent_disk_data_seek_to() {
        let dirent = DirectoryEntryDiskData {
            data: DiskRegion { sector: 1, size: 2 },
            attributes: DirentAttributes(0),
            filename_length: 0,
        };

        let mut seeker = Cursor::new(Vec::new());
        let result = dirent.seek_to(&mut seeker).expect("Seek should succeed");
        assert_eq!(result, 2048);
    }
}

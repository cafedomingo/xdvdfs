/// Trait for read operations on some block device containing an XDVDFS filesystem
pub trait BlockDeviceRead {
    type ReadError: core::error::Error + Send + Sync + 'static;

    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Self::ReadError>;
}

impl<R> BlockDeviceRead for R
where
    R: std::io::Read + std::io::Seek,
{
    type ReadError = std::io::Error;

    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), std::io::Error> {
        self.seek(std::io::SeekFrom::Start(offset))?;
        std::io::Read::read_exact(self, buffer)?;

        Ok(())
    }
}

#[cfg(test)]
mod test_io_impl {
    use std::io::Cursor;

    use super::BlockDeviceRead;

    #[test]
    fn test_blockdev_read_std_read_impl() {
        let mut cursor = Cursor::new(&[1, 2, 3, 4, 5]);
        let mut buf = [0, 0, 0];

        let res = BlockDeviceRead::read(&mut cursor, 1, &mut buf);
        assert!(res.is_ok());
        assert_eq!(buf, [2, 3, 4]);
    }
}

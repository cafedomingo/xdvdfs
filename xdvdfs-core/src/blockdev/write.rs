/// Trait for write operations on some block device
pub trait BlockDeviceWrite {
    type WriteError: core::error::Error + Send + Sync + 'static;

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<(), Self::WriteError>;
    fn len(&mut self) -> Result<u64, Self::WriteError>;

    fn is_empty(&mut self) -> Result<bool, Self::WriteError> {
        self.len().map(|len| len == 0)
    }
}

impl<W> BlockDeviceWrite for W
where
    W: std::io::Write + std::io::Seek,
{
    type WriteError = std::io::Error;

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<(), std::io::Error> {
        self.seek(std::io::SeekFrom::Start(offset))?;
        std::io::Write::write_all(self, buffer)?;

        Ok(())
    }

    fn len(&mut self) -> Result<u64, std::io::Error> {
        use std::io::SeekFrom;

        let current_position = self.stream_position()?;
        let len = self.seek(SeekFrom::End(0))?;
        self.seek(SeekFrom::Start(current_position))?;

        Ok(len)
    }
}

#[cfg(test)]
mod test_io_impl {
    use std::io::Cursor;

    use super::BlockDeviceWrite;

    #[test]
    fn test_blockdev_write_std_read_impl() {
        let mut cursor = Cursor::new(Vec::new());
        let buf = [1, 2, 3, 4];

        let res = BlockDeviceWrite::is_empty(&mut cursor);
        assert!(res.is_ok_and(|empty| empty));

        let res = BlockDeviceWrite::write(&mut cursor, 0, &buf);
        assert!(res.is_ok());
        assert_eq!(cursor.get_ref(), &[1, 2, 3, 4]);

        let res = BlockDeviceWrite::write(&mut cursor, 2, &buf);
        assert!(res.is_ok());
        assert_eq!(cursor.get_ref(), &[1, 2, 1, 2, 3, 4]);

        let len = BlockDeviceWrite::len(&mut cursor);
        assert!(len.is_ok_and(|len| len == 6));
    }
}

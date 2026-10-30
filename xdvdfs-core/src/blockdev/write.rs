use std::io;

/// Trait for write operations on some block device
#[allow(clippy::len_without_is_empty)]
pub trait BlockDeviceWrite {
    fn write(&mut self, offset: u64, buffer: &[u8]) -> io::Result<()>;
    fn len(&mut self) -> io::Result<u64>;
}

impl<W> BlockDeviceWrite for W
where
    W: io::Write + io::Seek,
{
    fn write(&mut self, offset: u64, buffer: &[u8]) -> io::Result<()> {
        self.seek(io::SeekFrom::Start(offset))?;
        io::Write::write_all(self, buffer)
    }

    fn len(&mut self) -> io::Result<u64> {
        let current_position = self.stream_position()?;
        let len = self.seek(io::SeekFrom::End(0))?;
        self.seek(io::SeekFrom::Start(current_position))?;

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

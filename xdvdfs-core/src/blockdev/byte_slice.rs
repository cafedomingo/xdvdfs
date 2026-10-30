use std::io;

use super::{BlockDeviceRead, BlockDeviceWrite};

impl BlockDeviceRead for [u8] {
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let offset = offset as usize;
        let source_len = self.as_ref().len();
        let size = <[u8]>::len(buffer);
        if offset >= source_len || source_len - offset < size {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        let range = offset..(offset + size);
        buffer.copy_from_slice(&self[range]);
        Ok(())
    }
}

impl BlockDeviceWrite for [u8] {
    fn write(&mut self, offset: u64, buffer: &[u8]) -> io::Result<()> {
        let offset: usize = offset.try_into().map_err(|_| io::ErrorKind::WriteZero)?;
        let buffer_size = <[u8]>::len(self);
        if offset >= buffer_size || buffer_size - offset < buffer.len() {
            return Err(io::ErrorKind::WriteZero.into());
        }

        self[offset..(offset + buffer.len())].copy_from_slice(buffer);
        Ok(())
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(<[u8]>::len(self) as u64)
    }
}

#[cfg(test)]
mod test {
    use std::io::ErrorKind;

    use crate::blockdev::{BlockDeviceRead, BlockDeviceWrite};

    #[test]
    fn test_blockdev_byte_slice_read_offset_out_of_range() {
        let mut bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        let res = bytes.as_mut_slice().read(6, &mut buffer);
        assert!(res.is_err_and(|err| err.kind() == ErrorKind::UnexpectedEof));
    }

    #[test]
    fn test_blockdev_byte_slice_read_size_out_of_range() {
        let mut bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        let res = bytes.as_mut_slice().read(0, &mut buffer);
        assert!(res.is_err_and(|err| err.kind() == ErrorKind::UnexpectedEof));
    }

    #[test]
    fn test_blockdev_byte_slice_read_in_bounds() {
        let mut bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        bytes
            .as_mut_slice()
            .read(1, &mut buffer[..3])
            .expect("Read should succeed");
        assert_eq!(buffer[..3], [2, 3, 4]);
    }

    #[test]
    fn test_blockdev_byte_slice_write_offset_out_of_range() {
        let bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        let res = buffer.as_mut_slice().write(105, &bytes);
        assert!(res.is_err_and(|err| err.kind() == ErrorKind::WriteZero));
    }

    #[test]
    fn test_blockdev_byte_slice_write_size_out_of_range() {
        let bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        let res = buffer.as_mut_slice().write(98, &bytes);
        assert!(res.is_err_and(|err| err.kind() == ErrorKind::WriteZero));
    }

    #[test]
    fn test_blockdev_byte_slice_write_in_bounds() {
        let bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        buffer
            .as_mut_slice()
            .write(50, &bytes)
            .expect("Write should succeed");
        assert_eq!(buffer[50..55], [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_blockdev_byte_slice_write_len() {
        let bytes = [1, 2, 3, 4, 5];
        let mut buffer = [0u8; 100];

        buffer
            .as_mut_slice()
            .write(50, &bytes)
            .expect("Write should succeed");
        assert_eq!(buffer[50..55], [1, 2, 3, 4, 5]);

        let len = buffer
            .as_mut_slice()
            .len()
            .expect("Len should be computed without error");
        assert_eq!(len, 100);
    }

    #[test]
    fn test_blockdev_byte_slice_write_boxed_in_bounds() {
        let bytes = [1, 2, 3, 4, 5];
        let mut buffer = Box::new([0u8; 100]);

        buffer.write(50, &bytes).expect("Write should succeed");
        assert_eq!(buffer[50..55], [1, 2, 3, 4, 5]);

        let len = BlockDeviceWrite::len(buffer.as_mut_slice())
            .expect("Len should be computed without error");
        assert_eq!(len, 100);
    }
}

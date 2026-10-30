use super::BlockDeviceWrite;

/// Block device that eats all write operations, without performing any writes.
/// Used for benchmarking. len() will return the correct value based on any write
/// ops given to the device, but the writes are not persisted and operations return
/// immediately.
#[derive(Default, Copy, Clone)]
pub struct NullBlockDevice {
    size: u64,
}

impl NullBlockDevice {
    pub fn write_size_adjustment(&mut self, offset: u64, size: u64) {
        self.size = core::cmp::max(self.size, offset + size);
    }

    pub fn len_blocking(&self) -> u64 {
        self.size
    }
}

impl BlockDeviceWrite for NullBlockDevice {
    type WriteError = core::convert::Infallible;

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<(), Self::WriteError> {
        self.write_size_adjustment(offset, buffer.len() as u64);
        Ok(())
    }

    fn len(&mut self) -> Result<u64, Self::WriteError> {
        Ok(self.size)
    }
}

use std::io;

use crate::blockdev::{BlockDeviceRead, BlockDeviceWrite};

/// Copy specialization for underlying XDVDFSFilesystem block devices
/// The default implementation of `copy` makes no assumptions about the
/// block devices and performs a buffered copy between them.
/// Override this trait if making assumptions about your block devices
/// allows for optimizing copies between them.
pub trait RWCopier<R, W>: Default
where
    R: BlockDeviceRead + ?Sized,
    W: BlockDeviceWrite + ?Sized,
{
    fn copy(
        &mut self,
        offset_in: u64,
        offset_out: u64,
        size: u64,
        src: &mut R,
        dest: &mut W,
    ) -> io::Result<u64>;
}

/// Default copier specialization, uses the default
/// RWCopier implementation for all inputs
pub struct DefaultCopier<R: ?Sized, W: ?Sized> {
    buffer: Box<[u8]>,
    r_type: core::marker::PhantomData<R>,
    w_type: core::marker::PhantomData<W>,
}

impl<R: ?Sized, W: ?Sized> Default for DefaultCopier<R, W> {
    fn default() -> Self {
        Self {
            buffer: vec![0u8; 1024 * 1024].into_boxed_slice(),
            r_type: core::marker::PhantomData,
            w_type: core::marker::PhantomData,
        }
    }
}

impl<R, W> RWCopier<R, W> for DefaultCopier<R, W>
where
    R: BlockDeviceRead + ?Sized,
    W: BlockDeviceWrite + ?Sized,
{
    fn copy(
        &mut self,
        offset_in: u64,
        offset_out: u64,
        size: u64,
        src: &mut R,
        dest: &mut W,
    ) -> io::Result<u64> {
        let buf_size = self.buffer.len() as u64;
        let mut copied = 0;
        while copied < size {
            let to_copy = core::cmp::min(buf_size, size - copied);
            let slice = &mut self.buffer[0..(to_copy as usize)];

            src.read(offset_in + copied, slice)?;
            dest.write(offset_out + copied, slice)?;
            copied += to_copy;
        }

        assert_eq!(copied, size);
        Ok(copied)
    }
}

/// Copier specialization for std::io block devices.
/// This applies to block devices that implement Read, Seek, and Write,
/// or block devices that implement the above and are wrapped by
/// `xdvdfs::blockdev::OffsetWrapper` and specializes the copy to use
/// `std::io::copy`
pub struct StdIOCopier<R: ?Sized, W: ?Sized> {
    r_type: core::marker::PhantomData<R>,
    w_type: core::marker::PhantomData<W>,
}

impl<R: ?Sized, W: ?Sized> Default for StdIOCopier<R, W> {
    fn default() -> Self {
        Self {
            r_type: core::marker::PhantomData,
            w_type: core::marker::PhantomData,
        }
    }
}

impl<R, W> RWCopier<R, W> for StdIOCopier<R, W>
where
    R: BlockDeviceRead + io::Read + io::Seek + Sized,
    W: BlockDeviceWrite + io::Write + io::Seek + ?Sized,
{
    fn copy(
        &mut self,
        offset_in: u64,
        offset_out: u64,
        size: u64,
        src: &mut R,
        dest: &mut W,
    ) -> io::Result<u64> {
        use std::io::{Read, SeekFrom};
        src.seek(SeekFrom::Start(offset_in))?;
        dest.seek(SeekFrom::Start(offset_out))?;
        std::io::copy(&mut src.by_ref().take(size), dest)
    }
}

impl<R, W> RWCopier<crate::blockdev::OffsetWrapper<R>, W>
    for StdIOCopier<crate::blockdev::OffsetWrapper<R>, W>
where
    R: BlockDeviceRead + io::Read + io::Seek + Sized,
    W: BlockDeviceWrite + io::Write + io::Seek + ?Sized,
{
    fn copy(
        &mut self,
        offset_in: u64,
        offset_out: u64,
        size: u64,
        src: &mut crate::blockdev::OffsetWrapper<R>,
        dest: &mut W,
    ) -> io::Result<u64> {
        use std::io::{Read, Seek, SeekFrom};
        src.seek(SeekFrom::Start(offset_in))?;
        dest.seek(SeekFrom::Start(offset_out))?;
        std::io::copy(&mut src.get_mut().by_ref().take(size), dest)
    }
}

#[cfg(test)]
mod test {
    use super::{DefaultCopier, RWCopier};

    #[test]
    fn test_write_xdvdfs_default_copier() {
        let mut dest = vec![0u8; 20];
        let mut src = vec![0xffu8; 20];
        src[0..3].fill(0x2e);

        // Smaller than the copy, so it takes several rounds
        let mut copier = DefaultCopier {
            buffer: vec![0; 5].into_boxed_slice(),
            ..DefaultCopier::default()
        };
        let res = copier.copy(1, 2, 20 - 2, src.as_mut_slice(), dest.as_mut_slice());
        assert_eq!(res.expect("Copy should succeed"), 20 - 2);
        assert_eq!(dest[0..2], [0, 0]);
        assert_eq!(src[1..19], dest[2..]);
    }
}

#[cfg(test)]
mod test_std {
    use std::io::Cursor;

    use crate::{
        blockdev::{OffsetWrapper, XDVDFSOffsets},
        write::fs::{RWCopier, StdIOCopier},
    };

    #[test]
    fn test_write_xdvdfs_std_copier() {
        let dest = std::vec![0u8; 20];
        let mut dest = Cursor::new(dest);

        let mut src = vec![0xffu8; 20];
        src[0..3].fill(0x2e);
        let mut src = Cursor::new(src);

        let mut copier = StdIOCopier::default();
        let res = copier.copy(1, 2, 20 - 2, &mut src, &mut dest);
        assert!(res.is_ok_and(|sz| sz == 20 - 2));
        assert_eq!(dest.get_ref()[0..2], [0, 0]);
        assert_eq!(src.get_ref()[1..19], dest.get_ref()[2..]);
    }

    #[test]
    fn test_write_xdvdfs_std_copier_offset_wrapper() {
        let dest = std::vec![0u8; 20];
        let mut dest = Cursor::new(dest);

        let mut src = vec![0xffu8; 20 + XDVDFSOffsets::XGD3 as usize];
        src[0..(3 + XDVDFSOffsets::XGD3 as usize)].fill(0x2e);
        let src = Cursor::new(src);
        let mut src = OffsetWrapper {
            inner: src,
            offset: XDVDFSOffsets::XGD3,
        };

        let mut copier = StdIOCopier::default();
        let res = copier.copy(1, 2, 20 - 2, &mut src, &mut dest);
        assert!(res.is_ok_and(|sz| sz == 20 - 2));
        assert_eq!(dest.get_ref()[0..2], [0, 0]);

        let start = XDVDFSOffsets::XGD3 as usize + 1;
        assert_eq!(
            src.get_ref().get_ref()[start..(start + 18)],
            dest.get_ref()[2..]
        );
    }
}

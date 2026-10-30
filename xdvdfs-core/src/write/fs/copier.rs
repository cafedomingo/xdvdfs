use core::ops::DerefMut;

use super::PathRef;
use crate::blockdev::BlockDeviceWrite;

/// A trait for copying data out of a filesystem.
///
/// Allows for copying data from a specified filesystem path
/// into an output block device, specialized by the block device type.
/// Multiple implementations of this trait allow the filesystem to be
/// used to create images on various output types.
pub trait FilesystemCopier<BDW: BlockDeviceWrite + ?Sized> {
    /// Copy the entire contents of file `src` into `dest` at the specified offset
    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64>;
}

impl<BDW, F, FDeref> FilesystemCopier<BDW> for FDeref
where
    BDW: BlockDeviceWrite + ?Sized,
    F: FilesystemCopier<BDW> + ?Sized,
    FDeref: DerefMut<Target = F>,
{
    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        self.deref_mut()
            .copy_file_in(src, dest, input_offset, output_offset, size)
    }
}

#[cfg(test)]
mod test {
    use crate::{
        blockdev::BlockDeviceWrite,
        write::fs::{FilesystemCopier, MemoryFilesystem, PathRef},
    };

    struct FSContainer<F>(F);

    impl<BDW: BlockDeviceWrite + ?Sized, F: FilesystemCopier<BDW>> FilesystemCopier<BDW>
        for FSContainer<F>
    {
        fn copy_file_in(
            &mut self,
            src: PathRef<'_>,
            dest: &mut BDW,
            input_offset: u64,
            output_offset: u64,
            size: u64,
        ) -> crate::Result<u64> {
            self.0
                .copy_file_in(src, dest, input_offset, output_offset, size)
        }
    }

    #[test]
    fn test_fs_copier_boxed_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a", b"abc");

        let memfs = Box::new(memfs);
        let mut fs = FSContainer(memfs);

        let mut buf = [0u8; 3];
        let res = fs.copy_file_in("/a".into(), buf.as_mut_slice(), 0, 0, 3);
        assert_eq!(res.expect("Copy should succeed"), 3);
        assert_eq!(&buf, b"abc");
    }

    #[test]
    fn test_fs_copier_ref_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a", b"abc");

        let mut memfs = Box::new(memfs);
        let mut fs = FSContainer(&mut memfs);

        let mut buf = [0u8; 3];
        let res = fs.copy_file_in("/a".into(), buf.as_mut_slice(), 0, 0, 3);
        assert_eq!(res.expect("Copy should succeed"), 3);
        assert_eq!(&buf, b"abc");
    }
}

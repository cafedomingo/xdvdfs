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
    type Error: core::error::Error + Send + Sync + 'static;

    /// Copy the entire contents of file `src` into `dest` at the specified offset
    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, Self::Error>;
}

impl<BDW, F, FDeref> FilesystemCopier<BDW> for FDeref
where
    BDW: BlockDeviceWrite + ?Sized,
    F: FilesystemCopier<BDW> + ?Sized,
    FDeref: DerefMut<Target = F>,
{
    type Error = F::Error;

    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> Result<u64, Self::Error> {
        self.deref_mut()
            .copy_file_in(src, dest, input_offset, output_offset, size)
    }
}

#[cfg(test)]
mod test {
    use crate::{
        blockdev::{BlockDeviceWrite, NullBlockDevice},
        write::fs::{FilesystemCopier, MemoryFilesystem, PathRef},
    };

    struct FSContainer<F>(F);

    impl<BDW: BlockDeviceWrite, F: FilesystemCopier<BDW>> FilesystemCopier<BDW> for FSContainer<F> {
        type Error = F::Error;

        fn copy_file_in(
            &mut self,
            src: PathRef<'_>,
            dest: &mut BDW,
            input_offset: u64,
            output_offset: u64,
            size: u64,
        ) -> Result<u64, Self::Error> {
            self.0
                .copy_file_in(src, dest, input_offset, output_offset, size)
        }
    }

    #[test]
    fn test_fs_copier_boxed_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let memfs = Box::new(memfs);
        let mut fs = FSContainer(memfs);

        let mut nullbd = NullBlockDevice::default();

        let res = fs.copy_file_in("/a".into(), &mut nullbd, 0, 0, 123);
        assert_eq!(res, Ok(123));
    }

    #[test]
    fn test_fs_copier_ref_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let mut memfs = Box::new(memfs);
        let mut fs = FSContainer(&mut memfs);

        let mut nullbd = NullBlockDevice::default();

        let res = fs.copy_file_in("/a".into(), &mut nullbd, 0, 0, 123);
        assert_eq!(res, Ok(123));
    }
}

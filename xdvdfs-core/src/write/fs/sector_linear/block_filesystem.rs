use std::io;

use crate::write::fs::{
    FileEntry, FilesystemCopier, FilesystemHierarchy, PathRef, SectorLinearBlockDevice,
    SectorLinearBlockRegion,
};

pub struct SectorLinearBlockFilesystem<F> {
    pub fs: F,
}

impl<F> SectorLinearBlockFilesystem<F>
where
    F: FilesystemHierarchy + FilesystemCopier<[u8]>,
{
    pub fn new(fs: F) -> Self {
        Self { fs }
    }
}

impl<F> FilesystemHierarchy for SectorLinearBlockFilesystem<F>
where
    F: FilesystemHierarchy,
{
    fn read_dir(&mut self, path: PathRef<'_>) -> crate::Result<Vec<FileEntry>> {
        self.fs.read_dir(path)
    }

    fn clear_cache(&mut self) -> crate::Result<()> {
        self.fs.clear_cache()
    }
}

impl<F> FilesystemCopier<SectorLinearBlockDevice> for SectorLinearBlockFilesystem<F>
where
    F: FilesystemCopier<[u8]>,
{
    fn copy_file_in(
        &mut self,
        src: PathRef<'_>,
        dest: &mut SectorLinearBlockDevice,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        if input_offset != 0 {
            let what = format!("{src}: can't copy from offset {input_offset}, only from the start");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, what).into());
        }

        let sector = SectorLinearBlockDevice::sector_at(output_offset)?;
        let region = SectorLinearBlockRegion::File {
            path: src.into(),
            len: size,
        };
        dest.insert(sector, region)?;
        Ok(size)
    }
}

#[cfg(test)]
mod test {
    use std::io::ErrorKind;

    use crate::{
        write::fs::{FilesystemCopier, MemoryFilesystem, PathRef, SectorLinearBlockSectorContents},
        Error,
    };

    use super::{SectorLinearBlockDevice, SectorLinearBlockFilesystem};

    #[test]
    fn test_sector_linear_fs_copier() {
        let memfs = MemoryFilesystem::default();
        let mut slbfs = SectorLinearBlockFilesystem::new(memfs);
        let mut slbd = SectorLinearBlockDevice::default();

        let path: PathRef = "/a/b".into();

        let copied = slbfs
            .copy_file_in(path, &mut slbd, 0, 2048, 5000)
            .expect("Copy should succeed");
        assert_eq!(copied, 5000);

        assert_eq!(slbd.get(0), SectorLinearBlockSectorContents::Empty);
        assert_eq!(slbd.get(1), SectorLinearBlockSectorContents::File(path));
        assert_eq!(slbd.get(2), SectorLinearBlockSectorContents::File(path));
        assert_eq!(slbd.get(3), SectorLinearBlockSectorContents::File(path));
        assert_eq!(slbd.get(4), SectorLinearBlockSectorContents::Empty);
    }

    #[test]
    fn test_sector_linear_fs_copier_rejects_bad_offsets() {
        let mut slbfs = SectorLinearBlockFilesystem::new(MemoryFilesystem::default());
        let mut slbd = SectorLinearBlockDevice::default();
        slbfs
            .copy_file_in("/a".into(), &mut slbd, 0, 2048, 5000)
            .expect("Copy should succeed");

        // A nonzero input offset, an unaligned output offset, and overlapping `/a`
        for (input_offset, output_offset) in [(1, 8192), (0, 8193), (0, 6144)] {
            let res = slbfs.copy_file_in("/b".into(), &mut slbd, input_offset, output_offset, 1);
            assert!(
                matches!(&res, Err(Error::Io(err)) if err.kind() == ErrorKind::InvalidInput),
                "copy from {input_offset} to {output_offset}: {res:?}"
            );
        }
        assert_eq!(slbd.num_sectors(), 4);
    }
}

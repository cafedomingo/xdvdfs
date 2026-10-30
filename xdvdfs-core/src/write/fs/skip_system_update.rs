use super::{FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, ImagePath};
use crate::blockdev::BlockDeviceWrite;

/// The folder on retail discs that holds the dashboard update
pub const SYSTEM_UPDATE_DIR: &str = "$SystemUpdate";

pub fn is_system_update_dir(name: &str) -> bool {
    name.eq_ignore_ascii_case(SYSTEM_UPDATE_DIR)
}

/// A source without its `$SystemUpdate` folders if `skip`, else the source itself
pub struct SkipSystemUpdateFilesystem<F> {
    inner: F,
    skip: bool,
}

impl<F> SkipSystemUpdateFilesystem<F> {
    pub fn new(inner: F, skip: bool) -> Self {
        Self { inner, skip }
    }
}

impl<F: FilesystemHierarchy> FilesystemHierarchy for SkipSystemUpdateFilesystem<F> {
    fn read_dir(&mut self, dir: &ImagePath) -> crate::Result<Vec<FileEntry>> {
        let mut entries = self.inner.read_dir(dir)?;
        if self.skip {
            entries.retain(|entry| {
                entry.file_type != FileType::Directory || !is_system_update_dir(&entry.name)
            });
        }
        Ok(entries)
    }
}

impl<BDW, F> FilesystemCopier<BDW> for SkipSystemUpdateFilesystem<F>
where
    BDW: BlockDeviceWrite + ?Sized,
    F: FilesystemCopier<BDW>,
{
    fn copy_file_in(
        &mut self,
        src: &ImagePath,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        self.inner
            .copy_file_in(src, dest, input_offset, output_offset, size)
    }
}

#[cfg(test)]
mod test {
    use crate::write::fs::{MemoryFilesystem, SkipSystemUpdateFilesystem, dir_tree};

    #[test]
    fn test_skips_system_update_at_any_depth() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/default.xbe", b"xbe");
        memfs.create("/$SystemUpdate/su20076000_00000000", b"update");
        memfs.create("/sub/$SYSTEMUPDATE/x", b"x");
        memfs.create("/sub/kept", b"kept");
        // Only a directory is left out
        memfs.create("/files/$SystemUpdate", b"file");
        let mut fs = SkipSystemUpdateFilesystem::new(memfs, true);

        let tree = dir_tree(&mut fs, &mut |_| {}).unwrap();
        let listed: Vec<_> = tree
            .iter()
            .flat_map(|dir| {
                let names = dir.listing.iter().map(|(entry, _)| entry.name.as_str());
                names.map(|name| dir.dir.join(name).to_string())
            })
            .collect();
        assert_eq!(
            listed,
            [
                "/default.xbe",
                "/files",
                "/sub",
                "/files/$SystemUpdate",
                "/sub/kept",
            ]
        );
    }
}

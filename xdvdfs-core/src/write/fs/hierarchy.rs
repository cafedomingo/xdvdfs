use core::ops::DerefMut;
use std::collections::VecDeque;

use super::{PathRef, PathVec};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileType {
    File,
    Directory,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileEntry {
    pub name: String,
    pub file_type: FileType,
    pub len: u64,
}

#[derive(Clone, Debug)]
pub struct DirectoryTreeEntry {
    pub dir: PathVec,
    pub listing: Vec<(FileEntry, usize)>,
}

/// Returns a recursive listing of paths in order
/// e.g. for a path hierarchy like this:
/// /
/// -- /a
/// -- -- /a/b
/// -- /b
/// It might return the list: ["/", "/a", "/a/b", "/b"]
/// `directory_found_callback` should be called each time
/// a new directory is found, with the number of entries
/// in that directory (for progress tracking).
pub fn dir_tree<FS: FilesystemHierarchy + ?Sized>(
    fs: &mut FS,
    directory_found_callback: &mut impl FnMut(usize),
) -> Result<Vec<DirectoryTreeEntry>, FS::Error> {
    let mut dirs: VecDeque<PathVec> = VecDeque::new();
    dirs.push_back(PathVec::default());
    let mut out = Vec::new();

    while let Some(dir) = dirs.pop_front() {
        let entries = fs.read_dir(dir.as_path_ref())?;
        directory_found_callback(entries.len());
        let mut listing: Vec<(FileEntry, usize)> = Vec::with_capacity(entries.len());

        let current_dir_index = out.len();

        for entry in entries.into_iter() {
            let mut dir_index: usize = 0;
            if let FileType::Directory = entry.file_type {
                dirs.push_back(PathVec::from_base(dir.clone(), &entry.name));

                // This directory is in position `dirs.len()` in the queue,
                // so it will be that many indices past the current directory.
                dir_index = current_dir_index + dirs.len();
            }

            listing.push((entry, dir_index));
        }

        out.push(DirectoryTreeEntry { dir, listing });
    }

    Ok(out)
}

/// A trait for filesystem hierarchies, representing any filesystem
/// structure with hierarchical directories.
///
/// This trait allows for scanning a given directory within a filesystem
/// for a list of its entries and entry metadata.
pub trait FilesystemHierarchy {
    type Error: core::error::Error + 'static;

    /// Read a directory, and return a list of entries within it
    fn read_dir(&mut self, path: PathRef<'_>) -> Result<Vec<FileEntry>, Self::Error>;

    /// Clear any cached data built during operation
    /// After clearing the cache, function should behave as though the object
    /// has just been initialized.
    fn clear_cache(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<F, FDeref> FilesystemHierarchy for FDeref
where
    F: FilesystemHierarchy,
    FDeref: DerefMut<Target = F>,
{
    type Error = F::Error;

    fn read_dir(&mut self, path: PathRef<'_>) -> Result<Vec<FileEntry>, Self::Error> {
        self.deref_mut().read_dir(path)
    }

    fn clear_cache(&mut self) -> Result<(), Self::Error> {
        self.deref_mut().clear_cache()
    }
}

#[cfg(test)]
mod test {
    use crate::write::fs::{FilesystemHierarchy, MemoryFilesystem, PathRef};

    use super::FileEntry;

    struct FSContainer<F: FilesystemHierarchy>(F);

    impl<F: FilesystemHierarchy> FilesystemHierarchy for FSContainer<F> {
        type Error = F::Error;

        fn read_dir(&mut self, path: PathRef<'_>) -> Result<Vec<FileEntry>, Self::Error> {
            self.0.read_dir(path)
        }

        fn clear_cache(&mut self) -> Result<(), Self::Error> {
            self.0.clear_cache()
        }
    }

    #[test]
    fn test_fs_hierarchy_boxed_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let memfs = Box::new(memfs);
        let mut fs = FSContainer(memfs);

        let res = fs.clear_cache();
        assert_eq!(res, Ok(()));

        let res = fs.read_dir("/".into());
        assert_eq!(
            res,
            Ok([FileEntry {
                name: "a".into(),
                file_type: crate::write::fs::FileType::File,
                len: 0,
            },]
            .as_slice()
            .to_vec()),
        );
    }

    #[test]
    fn test_fs_hierarchy_ref_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let mut memfs = Box::new(memfs);
        let mut fs = FSContainer(&mut memfs);

        let res = fs.clear_cache();
        assert_eq!(res, Ok(()));

        let res = fs.read_dir("/".into());
        assert_eq!(
            res,
            Ok([FileEntry {
                name: "a".into(),
                file_type: crate::write::fs::FileType::File,
                len: 0,
            },]
            .as_slice()
            .to_vec()),
        );
    }
}

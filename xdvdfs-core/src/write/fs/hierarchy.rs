use core::ops::DerefMut;
use std::collections::VecDeque;

use super::ImagePath;
use crate::layout::cmp_ignore_case_utf8;

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
    pub dir: ImagePath,
    pub listing: Vec<(FileEntry, usize)>,
}

/// Returns every directory breadth-first, each listing sorted with its directory
/// table's name comparison. For directories `/a`, `/a/b` and `/B`, that's `/`,
/// `/a`, `/B`, `/a/b`.
/// `directory_found_callback` should be called each time
/// a new directory is found, with the number of entries
/// in that directory (for progress tracking).
pub fn dir_tree<FS: FilesystemHierarchy + ?Sized>(
    fs: &mut FS,
    directory_found_callback: &mut impl FnMut(usize),
) -> crate::Result<Vec<DirectoryTreeEntry>> {
    let mut dirs: VecDeque<ImagePath> = VecDeque::new();
    dirs.push_back(ImagePath::root());
    let mut out = Vec::new();

    while let Some(dir) = dirs.pop_front() {
        let mut entries = fs.read_dir(&dir)?;
        // Traversal order decides where each directory's entries are allocated
        entries.sort_by(|a, b| cmp_ignore_case_utf8(&a.name, &b.name));
        directory_found_callback(entries.len());
        let mut listing: Vec<(FileEntry, usize)> = Vec::with_capacity(entries.len());

        let current_dir_index = out.len();

        for entry in entries.into_iter() {
            let mut dir_index: usize = 0;
            if let FileType::Directory = entry.file_type {
                dirs.push_back(dir.join(&entry.name));

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
    /// Read a directory, and return a list of entries within it
    fn read_dir(&mut self, path: &ImagePath) -> crate::Result<Vec<FileEntry>>;
}

impl<F, FDeref> FilesystemHierarchy for FDeref
where
    F: FilesystemHierarchy,
    FDeref: DerefMut<Target = F>,
{
    fn read_dir(&mut self, path: &ImagePath) -> crate::Result<Vec<FileEntry>> {
        self.deref_mut().read_dir(path)
    }
}

#[cfg(test)]
mod test {
    use crate::write::fs::{FilesystemHierarchy, ImagePath, MemoryFilesystem};

    use super::FileEntry;

    struct FSContainer<F: FilesystemHierarchy>(F);

    impl<F: FilesystemHierarchy> FilesystemHierarchy for FSContainer<F> {
        fn read_dir(&mut self, path: &ImagePath) -> crate::Result<Vec<FileEntry>> {
            self.0.read_dir(path)
        }
    }

    #[test]
    fn test_fs_hierarchy_boxed_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let memfs = Box::new(memfs);
        let mut fs = FSContainer(memfs);

        let res = fs
            .read_dir(&ImagePath::root())
            .expect("read_dir should succeed");
        assert_eq!(
            res,
            [FileEntry {
                name: "a".into(),
                file_type: crate::write::fs::FileType::File,
                len: 0,
            }],
        );
    }

    #[test]
    fn test_fs_hierarchy_ref_impl() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a");

        let mut memfs = Box::new(memfs);
        let mut fs = FSContainer(&mut memfs);

        let res = fs
            .read_dir(&ImagePath::root())
            .expect("read_dir should succeed");
        assert_eq!(
            res,
            [FileEntry {
                name: "a".into(),
                file_type: crate::write::fs::FileType::File,
                len: 0,
            }],
        );
    }
}

use std::collections::BTreeMap;
use std::io::{self, ErrorKind};

use crate::{
    Error,
    write::fs::{FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, ImagePath},
};

#[derive(Debug, Clone)]
enum Entry {
    Directory,
    File(Vec<u8>),
}

fn out_of_bounds(kind: ErrorKind, offset: usize, size: usize, len: usize) -> io::Error {
    let what = format!(
        "access from offset {offset} of size {size} out of bounds for buffer of size {len}"
    );
    io::Error::new(kind, what)
}

#[derive(Default, Debug, Clone)]
pub struct MemoryFilesystem(BTreeMap<ImagePath, Entry>);

impl MemoryFilesystem {
    pub fn mkdir(&mut self, path: impl Into<ImagePath>) {
        self.insert(path.into(), Entry::Directory);
    }

    pub fn touch(&mut self, path: impl Into<ImagePath>) {
        self.create(path, &[]);
    }

    pub fn create(&mut self, path: impl Into<ImagePath>, data: &[u8]) {
        self.insert(path.into(), Entry::File(data.to_vec()));
    }

    /// Inserts `entry` at `path`, and directories at its missing parents
    fn insert(&mut self, path: ImagePath, entry: Entry) {
        let mut parent = path.parent();
        while let Some(dir) = parent.filter(|dir| !dir.is_root()) {
            parent = dir.parent();
            self.0.entry(dir).or_insert(Entry::Directory);
        }

        self.0.insert(path, entry);
    }

    pub fn lsdir(&self, path: &ImagePath) -> Option<Vec<FileEntry>> {
        if !path.is_root() && !matches!(self.0.get(path), Some(Entry::Directory)) {
            return None;
        }

        let entries = self
            .0
            .iter()
            .filter(|(child, _)| child.parent().as_ref() == Some(path))
            .map(|(child, entry)| {
                let (file_type, len) = match entry {
                    Entry::Directory => (FileType::Directory, 0),
                    Entry::File(data) => (FileType::File, data.len() as u64),
                };
                FileEntry {
                    name: child.file_name().unwrap_or_default().to_owned(),
                    file_type,
                    len,
                }
            })
            .collect();
        Some(entries)
    }

    pub fn read(&self, path: &ImagePath, offset: usize, buffer: &mut [u8]) -> crate::Result<usize> {
        let Some(Entry::File(file)) = self.0.get(path) else {
            return Err(Error::NotFound(path.clone()));
        };
        if offset >= file.len() {
            let (size, len) = (buffer.len(), file.len());
            return Err(out_of_bounds(ErrorKind::UnexpectedEof, offset, size, len).into());
        }

        let size = core::cmp::min(buffer.len(), file.len() - offset);
        let limit = offset + size;
        assert!(limit <= file.len());

        buffer[0..size].copy_from_slice(&file[offset..limit]);
        Ok(size)
    }

    fn copy_file_in_impl(
        &self,
        src: &ImagePath,
        dest: &mut [u8],
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        let input_offset = input_offset as usize;
        let output_offset = output_offset as usize;
        let size = size as usize;

        if dest.len() <= output_offset {
            let kind = ErrorKind::WriteZero;
            return Err(out_of_bounds(kind, output_offset, size, dest.len()).into());
        }
        let limit = core::cmp::min(dest.len(), output_offset + size);
        assert!(limit >= output_offset);

        let buffer = &mut dest[output_offset..limit];
        let bytes_read = self.read(src, input_offset, buffer)?;
        dest[(output_offset + bytes_read)..limit].fill(0);
        Ok((limit - output_offset) as u64)
    }
}

impl FilesystemHierarchy for MemoryFilesystem {
    fn read_dir(&mut self, path: &ImagePath) -> crate::Result<Vec<FileEntry>> {
        self.lsdir(path)
            .ok_or_else(|| Error::NotFound(path.clone()))
    }
}

impl FilesystemCopier<[u8]> for MemoryFilesystem {
    fn copy_file_in(
        &mut self,
        src: &ImagePath,
        dest: &mut [u8],
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        self.copy_file_in_impl(src, dest, input_offset, output_offset, size)
    }
}

#[cfg(test)]
mod test {
    use std::io::ErrorKind;

    use crate::{
        Error,
        write::fs::{FileEntry, FileType, ImagePath, MemoryFilesystem},
    };

    #[test]
    fn test_memfs_lsdir_no_entry() {
        let memfs = MemoryFilesystem::default();
        assert_eq!(memfs.lsdir(&"/a".into()), None);
    }

    #[test]
    fn test_memfs_mkdir() {
        let mut memfs = MemoryFilesystem::default();
        memfs.mkdir("/a/b");
        memfs.mkdir("/a/c");

        let entries = memfs
            .lsdir(&"/a".into())
            .expect("'/a' should contain records");
        assert_eq!(
            entries,
            [
                FileEntry {
                    name: "b".to_string(),
                    file_type: FileType::Directory,
                    len: 0,
                },
                FileEntry {
                    name: "c".to_string(),
                    file_type: FileType::Directory,
                    len: 0,
                },
            ]
        );
    }

    #[test]
    fn test_memfs_create() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[0, 1, 2]);
        memfs.create("/a/c", &[3, 4]);

        let entries = memfs
            .lsdir(&"/a".into())
            .expect("'/a' should contain records");
        assert_eq!(
            entries,
            [
                FileEntry {
                    name: "b".to_string(),
                    file_type: FileType::File,
                    len: 3,
                },
                FileEntry {
                    name: "c".to_string(),
                    file_type: FileType::File,
                    len: 2,
                },
            ]
        );
    }

    #[test]
    fn test_memfs_touch() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a/b");
        memfs.touch("/a/c");

        let entries = memfs
            .lsdir(&"/a".into())
            .expect("'/a' should contain records");
        assert_eq!(
            entries,
            [
                FileEntry {
                    name: "b".to_string(),
                    file_type: FileType::File,
                    len: 0,
                },
                FileEntry {
                    name: "c".to_string(),
                    file_type: FileType::File,
                    len: 0,
                },
            ]
        );
    }

    #[test]
    fn test_memfs_mixed_files_and_dirs() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/a/b");
        memfs.mkdir("/a/c");
        memfs.create("a/d", &[1]);

        let entries = memfs
            .lsdir(&"/a".into())
            .expect("'/a' should contain records");
        assert_eq!(
            entries,
            [
                FileEntry {
                    name: "b".to_string(),
                    file_type: FileType::File,
                    len: 0,
                },
                FileEntry {
                    name: "c".to_string(),
                    file_type: FileType::Directory,
                    len: 0,
                },
                FileEntry {
                    name: "d".to_string(),
                    file_type: FileType::File,
                    len: 1,
                }
            ]
        );
    }

    #[test]
    fn test_memfs_read_no_entry() {
        let memfs = MemoryFilesystem::default();
        let mut buf = [0; 5];
        let res = memfs.read(&"/a".into(), 0, &mut buf);
        assert!(matches!(res, Err(Error::NotFound(path)) if path == ImagePath::from("/a")));
    }

    #[test]
    fn test_memfs_read_file_full() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        assert_eq!(memfs.read(&"/a/b".into(), 0, &mut buf).unwrap(), 5);
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_memfs_read_file_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        assert_eq!(memfs.read(&"/a/b".into(), 2, &mut buf).unwrap(), 3);
        assert_eq!(buf, [3, 4, 5, 0, 0]);
    }

    #[test]
    fn test_memfs_read_file_invalid_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        let res = memfs.read(&"/a/b".into(), 6, &mut buf);
        assert!(matches!(res, Err(Error::Io(err)) if err.kind() == ErrorKind::UnexpectedEof));
    }

    #[test]
    fn test_memfs_read_directory_fails() {
        let mut memfs = MemoryFilesystem::default();
        memfs.mkdir("/a/b");

        let mut buf = [0; 5];
        let res = memfs.read(&"/a/b".into(), 2, &mut buf);
        assert!(matches!(res, Err(Error::NotFound(path)) if path == ImagePath::from("/a/b")));
    }

    #[test]
    fn test_memfs_copy_file_in() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        assert_eq!(
            memfs
                .copy_file_in_impl(&"/a/b".into(), &mut buf, 0, 0, 5)
                .unwrap(),
            5
        );
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_memfs_copy_file_in_output_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        // Read past end of file, extra space filled with 0
        let mut buf = [10; 9];
        assert_eq!(
            memfs
                .copy_file_in_impl(&"/a/b".into(), &mut buf, 0, 2, 7)
                .unwrap(),
            7
        );
        assert_eq!(buf, [10, 10, 1, 2, 3, 4, 5, 0, 0]);
    }

    #[test]
    fn test_memfs_copy_file_in_invalid_output_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        let res = memfs.copy_file_in_impl(&"/a/b".into(), &mut buf, 0, 6, 5);
        assert!(matches!(res, Err(Error::Io(err)) if err.kind() == ErrorKind::WriteZero));
    }

    #[test]
    fn test_memfs_copy_file_in_buffer_size_clamped() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        // Output offset by 2, input offset by 0, data after EOF is filled 0
        let mut buf = [10; 8];
        assert_eq!(
            memfs
                .copy_file_in_impl(&"/a/b".into(), &mut buf, 0, 2, 7)
                .unwrap(),
            6
        );
        assert_eq!(buf, [10, 10, 1, 2, 3, 4, 5, 0]);
    }

    #[test]
    fn test_memfs_copy_file_in_input_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        // Read is offset by 2, data after EOF is filled 0
        let mut buf = [1; 5];
        assert_eq!(
            memfs
                .copy_file_in_impl(&"/a/b".into(), &mut buf, 2, 0, 5)
                .unwrap(),
            5
        );
        assert_eq!(buf, [3, 4, 5, 0, 0]);
    }

    #[test]
    fn test_memfs_copy_file_in_invalid_input_offset() {
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/a/b", &[1, 2, 3, 4, 5]);

        let mut buf = [0; 5];
        let res = memfs.copy_file_in_impl(&"/a/b".into(), &mut buf, 5, 0, 5);
        assert!(matches!(res, Err(Error::Io(err)) if err.kind() == ErrorKind::UnexpectedEof));
    }
}

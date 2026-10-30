use std::io;

use crate::write::fs::ImagePath;

pub type Result<T> = std::result::Result<T, Error>;

#[non_exhaustive]
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("not an xdvdfs volume")]
    InvalidVolume,
    #[error("deserialization failed")]
    DeserializationFailed,
    #[error("offset {offset} out of bounds for file of size {size}")]
    OutOfBounds { offset: u64, size: u32 },
    #[error("unsafe file name {0:?}")]
    UnsafeName(String),
    #[error("directory entries loop or share a table")]
    Loop,
    #[error("duplicate file name {0:?}")]
    DuplicateName(String),
    #[error("directory is empty")]
    DirectoryEmpty,
    #[error("entry does not exist")]
    DoesNotExist,
    #[error("path exists, but dirent does not (likely root)")]
    NoDirent,
    #[error("expected directory, found file")]
    IsNotDirectory,
    #[error("{0}: file runs past the end of the image")]
    TruncatedFile(ImagePath),
    #[error("{0}: directory table runs past the end of the image")]
    TruncatedDirectory(ImagePath),
    #[error("path \"{0}\" not found")]
    NotFound(ImagePath),

    #[error("{0}: name matches another in its directory (Xbox names ignore case); rename it")]
    DuplicateFileName(ImagePath),
    #[error("too many entries in a single directory")]
    TooManyDirectoryEntries,
    #[error("{0}: file is too large for an Xbox image (must be under 4 GiB)")]
    FileTooLarge(ImagePath),
    #[error("{0}: name is longer than an Xbox image allows (255 Windows-1252 bytes); rename it")]
    FileNameTooLong(ImagePath),
    #[error("{0}: name can't be stored in an Xbox image (Windows-1252); rename it")]
    FileNameNotEncodable(ImagePath),
    #[error("{0}: name is empty, `.` or `..`, or contains `/`, `\\`, `:` or NUL; rename it")]
    InvalidFileName(ImagePath),
}

impl Error {
    /// Names `dir` if this is a short read of its table
    pub fn in_directory(self, dir: impl Into<ImagePath>) -> Self {
        match self {
            Self::Io(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                Self::TruncatedDirectory(dir.into())
            }
            err => err,
        }
    }

    /// Names `file` if this is a short read of its data
    pub fn in_file(self, file: impl Into<ImagePath>) -> Self {
        match self {
            Self::Io(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                Self::TruncatedFile(file.into())
            }
            err => err,
        }
    }
}

use ciso::read::CSOReader;
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use xdvdfs::blockdev::{BlockDeviceRead, OffsetWrapper};

pub struct CSOBlockDevice<R: ciso::read::Read<ReadError = std::io::Error>> {
    inner: CSOReader<std::io::Error, R>,
}

impl<R> BlockDeviceRead for CSOBlockDevice<R>
where
    R: ciso::read::Read<ReadError = std::io::Error>,
{
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), std::io::Error> {
        self.inner.read_offset(offset, buffer).map_err(|e| match e {
            ciso::layout::Error::Other(e) => e,
            e => std::io::Error::other(e),
        })
    }
}

pub fn open_image_raw(path: &Path) -> Result<OffsetWrapper<BufReader<File>>, anyhow::Error> {
    let img = File::options().read(true).open(path)?;
    let img = std::io::BufReader::new(img);
    Ok(xdvdfs::blockdev::OffsetWrapper::new(img)?)
}

pub fn open_image(path: &Path) -> Result<Box<dyn BlockDeviceRead>, anyhow::Error> {
    if path.extension().is_some_and(|e| e == "cso") {
        let file_base = path.with_extension("");
        let split = file_base.extension().is_some_and(|e| e == "1");

        let reader: Box<dyn ciso::read::Read<ReadError = std::io::Error>> = if split {
            let mut files = Vec::new();
            for i in 1.. {
                let part = file_base.with_extension(format!("{i}.cso"));
                if !part.exists() {
                    break;
                }

                let part = std::io::BufReader::new(std::fs::File::open(part)?);
                files.push(part);
            }

            if files.is_empty() {
                return Err(anyhow::anyhow!("Failed to open file {:?}", path));
            }

            Box::from(ciso::split::SplitFileReader::new(files)?)
        } else {
            let file = std::io::BufReader::new(std::fs::File::open(path)?);
            Box::from(file)
        };

        let reader = ciso::read::CSOReader::new(reader)?;
        let reader = Box::from(CSOBlockDevice { inner: reader });
        Ok(reader)
    } else {
        let image = open_image_raw(path)?;
        let image = Box::from(image);
        Ok(image)
    }
}

/// Names a truncated directory by its full path, since `file_tree` of the
/// table at `base` names them relative to it
pub fn under_dir(base: &str) -> impl Fn(xdvdfs::Error) -> xdvdfs::Error + '_ {
    move |err| match err {
        xdvdfs::Error::TruncatedDirectory(dir) => {
            xdvdfs::Error::TruncatedDirectory(base.split('/').chain(dir.iter()).collect())
        }
        err => err,
    }
}

/// Returns the result of std::fs::canonicalize, if it returned without error,
/// otherwise returns the result of std::path::absolute
/// std::fs::canonicalize will follow symlinks, but seems to error on some filesystems
/// on Windows. Following symlinks is desirable, so it is attempted first.
/// A path that doesn't exist yet, such as an output image, is resolved through
/// its parent directory, since std::path::absolute keeps `..` components.
pub fn absolute_path(path: &Path) -> std::io::Result<PathBuf> {
    if let Ok(path) = std::fs::canonicalize(path) {
        return Ok(path);
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match (std::fs::canonicalize(parent), path.file_name()) {
        (Ok(parent), Some(name)) => Ok(parent.join(name)),
        _ => std::path::absolute(path),
    }
}

/// Similar to Path::with_extension, but will not overwrite the extension for
/// directories
pub fn with_extension(path: &Path, ext: &str, is_dir: bool) -> PathBuf {
    if is_dir {
        path.with_added_extension(ext)
    } else {
        path.with_extension(ext)
    }
}

#[cfg(test)]
mod test {
    use super::with_extension;
    use std::path::Path;

    #[test]
    fn with_extension_not_dir() {
        assert_eq!(
            with_extension(Path::new("file.abc"), "xyz", false),
            Path::new("file.xyz")
        );
    }

    #[test]
    fn with_extension_dir_no_extension() {
        assert_eq!(
            with_extension(Path::new("dir"), "xyz", true),
            Path::new("dir.xyz")
        );
    }

    #[test]
    fn with_extension_dir_with_extension() {
        assert_eq!(
            with_extension(Path::new("dir.abc"), "xyz", true),
            Path::new("dir.abc.xyz")
        );
    }
}

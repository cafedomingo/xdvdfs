use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};
use xdvdfs::blockdev::{BlockDeviceRead, OffsetWrapper};
use xdvdfs::cso::CsoReader;
use xdvdfs::write::fs::ImagePath;

use crate::split::SplitFileReader;

pub fn open_image_raw(path: &Path) -> Result<OffsetWrapper<BufReader<File>>, anyhow::Error> {
    let img = File::options().read(true).open(path)?;
    let img = std::io::BufReader::new(img);
    Ok(xdvdfs::blockdev::OffsetWrapper::new(img)?)
}

/// Opens an image, or a CSO: `x.1.cso` with the parts after it, or any
/// other `x.cso` as one file
pub fn open_image(path: &Path) -> Result<Box<dyn BlockDeviceRead>, anyhow::Error> {
    if !path.extension().is_some_and(|e| e == "cso") {
        return Ok(Box::new(open_image_raw(path)?));
    }

    if path
        .with_extension("")
        .extension()
        .is_some_and(|e| e == "1")
    {
        let parts = SplitFileReader::open(path)?;
        let part_lens = parts.part_lens().to_vec();
        return Ok(Box::new(CsoReader::open(parts, &part_lens)?));
    }

    let file = File::open(path)?;
    let len = file.metadata()?.len();
    Ok(Box::new(CsoReader::open(file, &[len])?))
}

/// The full path of `path`, which `file_tree` of the table at `base` gives
/// relative to it
pub fn under(base: &ImagePath, path: &ImagePath) -> ImagePath {
    let mut full = base.clone();
    for name in path.components() {
        full.push(name);
    }
    full
}

/// Names a truncated directory by its full path, from `file_tree` of the table
/// at `base`
pub fn under_dir(base: &ImagePath) -> impl Fn(xdvdfs::Error) -> xdvdfs::Error + '_ {
    move |err| match err {
        xdvdfs::Error::TruncatedDirectory(dir) => {
            xdvdfs::Error::TruncatedDirectory(under(base, &dir))
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

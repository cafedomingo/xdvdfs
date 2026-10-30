use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::Args;
use xdvdfs::write::{
    fs::{StdFilesystem, StdIOCopier, XDVDFSFilesystem},
    img::create_xdvdfs_image,
};

use crate::{
    img::{absolute_path, with_extension},
    progress::StdIoProgressReporter,
};

#[derive(Args)]
#[command(about = "Pack an image from a given directory or source ISO image")]
pub struct PackArgs {
    #[arg(help = "Path to source directory or ISO image")]
    pub source_path: String,

    #[arg(help = "Path to output image")]
    pub image_path: Option<String>,
}

/// The output for `source_path` when none is given: its name with `ext`,
/// or with `xiso.<ext>` if that would be the source itself.
pub fn default_image_path(
    source_path: &Path,
    is_dir: bool,
    ext: &str,
) -> Result<PathBuf, anyhow::Error> {
    let source_file_name = source_path
        .file_name()
        .ok_or(anyhow::anyhow!("Failed to get file name from source path"))?;
    let output = with_extension(Path::new(source_file_name), ext, is_dir);

    if output.exists() && absolute_path(&output)? == source_path {
        return Ok(with_extension(
            Path::new(source_file_name),
            &format!("xiso.{ext}"),
            is_dir,
        ));
    }

    Ok(output)
}

/// Refuses an output that would overwrite or land inside the absolute
/// `source_path`, or that is a directory.
pub fn check_image_path(source_path: &Path, image_path: &Path) -> Result<(), anyhow::Error> {
    let image_path = absolute_path(image_path)?;
    if image_path == source_path {
        bail!(
            "Source and destination paths are the same: {}",
            image_path.display()
        );
    }

    if image_path.starts_with(source_path) {
        bail!(
            "Destination path is contained by source path: {}",
            image_path.display()
        );
    }

    if image_path.is_dir() {
        bail!("Destination path is a directory: {}", image_path.display());
    }

    Ok(())
}

/// The file that packing to `image_path` replaces: a symlink's target, so the
/// link survives.
fn output_path(image_path: &Path) -> PathBuf {
    let Ok(target) = std::fs::read_link(image_path) else {
        return image_path.to_owned();
    };

    // A dangling link has no target to canonicalize yet
    std::fs::canonicalize(image_path).unwrap_or_else(|_| image_path.with_file_name(target))
}

pub fn cmd_pack(args: &PackArgs) -> Result<(), anyhow::Error> {
    let source_path = absolute_path(Path::new(&args.source_path))?;
    let meta = std::fs::metadata(&source_path)?;
    let is_dir = meta.is_dir();

    let image_path = args
        .image_path
        .as_ref()
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| default_image_path(&source_path, is_dir, "iso"))?;

    cmd_pack_path(&source_path, &image_path)
}

pub fn cmd_pack_path(source_path: &Path, image_path: &Path) -> Result<(), anyhow::Error> {
    let image_path = output_path(image_path);
    let partial_path = image_path.with_added_extension("partial");
    for path in [&image_path, &partial_path] {
        check_image_path(source_path, path)?;
    }

    pack_into(source_path, &partial_path)?;
    if let Err(err) = std::fs::rename(&partial_path, &image_path) {
        let _ = std::fs::remove_file(&partial_path);
        return Err(err).with_context(|| format!("Failed to replace {}", image_path.display()));
    }

    Ok(())
}

/// Packs `source_path` into a new file at `image_path`, and removes that file
/// if packing fails.
pub fn pack_into(source_path: &Path, image_path: &Path) -> Result<(), anyhow::Error> {
    let result = write_image(source_path, image_path);
    if result.is_err() {
        // The packing error is the one to report
        let _ = std::fs::remove_file(image_path);
    }

    result
}

fn write_image(source_path: &Path, image_path: &Path) -> Result<(), anyhow::Error> {
    let meta = std::fs::metadata(source_path)?;
    let is_dir = meta.is_dir();

    let image = std::fs::File::create(image_path)?;
    let mut image = std::io::BufWriter::with_capacity(1024 * 1024, image);

    let progress_visitor = StdIoProgressReporter::new(source_path, is_dir);
    if is_dir {
        let mut fs = StdFilesystem::create(source_path);
        create_xdvdfs_image(&mut fs, &mut image, progress_visitor)?;
    } else if meta.is_file() {
        let source = crate::img::open_image_raw(source_path)?;
        let mut fs = XDVDFSFilesystem::<_, _, StdIOCopier<_, _>>::new(source)
            .ok_or(anyhow::anyhow!("Failed to create XDVDFS filesystem"))?;
        create_xdvdfs_image(&mut fs, &mut image, progress_visitor)?;
    } else {
        bail!("Source must be a directory or an image file");
    }

    // Dropping a `BufWriter` or `File` ignores errors, and some volumes report
    // write errors only when the data is synced
    image.flush()?;
    image.get_ref().sync_all()?;
    Ok(())
}

use std::{
    borrow::Cow,
    fs::{File, Metadata},
    io::{self, BufWriter},
    path::{Path, PathBuf},
};

use anyhow::bail;
use clap::Args;

use xdvdfs::write::{
    fs::{
        DefaultCopier, FilesystemCopier, FilesystemHierarchy, MediaPatchFilesystem,
        SectorLinearBlockDevice, SectorLinearBlockFilesystem, SectorLinearImage, StdFilesystem,
        XDVDFSFilesystem,
    },
    img::{ProgressInfo, create_xdvdfs_image},
};

use crate::cmd_pack::{MEDIA_PATCH_HELP, default_image_path};
use crate::img::{absolute_path, open_image_raw};
use crate::progress::print_progress;
use crate::split::{FATX_SPLIT_SIZE, SplitOutput};

#[derive(Args)]
#[command(about = "Pack and compress an image from a given directory or source ISO image")]
pub struct CompressArgs {
    #[arg(help = "Path to source directory or ISO image")]
    source_path: String,

    #[arg(help = "Path to output image")]
    image_path: Option<String>,

    #[arg(long, help = MEDIA_PATCH_HELP)]
    media_patch: bool,
}

pub fn cmd_compress(args: &CompressArgs) -> Result<(), anyhow::Error> {
    let source_path = absolute_path(Path::new(&args.source_path))?;
    let meta = std::fs::metadata(&source_path)?;
    let is_dir = meta.is_dir();

    let image_path = match &args.image_path {
        Some(image_path) => PathBuf::from(image_path),
        None => default_image_path(&source_path, is_dir, "cso")?,
    };

    let mut output = SplitOutput::new(&source_path, &image_path);
    match write_parts(&source_path, &meta, args.media_patch, &mut output) {
        Ok(parts) => output.finish(parts),
        Err(err) => {
            output.remove_partials();
            Err(err)
        }
    }
}

fn write_parts(
    source_path: &Path,
    meta: &Metadata,
    media_patch: bool,
    output: &mut SplitOutput,
) -> Result<Vec<BufWriter<File>>, anyhow::Error> {
    let source_prefix = if meta.is_dir() {
        source_path.to_string_lossy()
    } else {
        Cow::Borrowed("")
    };
    let progress = |pi: ProgressInfo| match pi {
        ProgressInfo::DirAdded(path, sector) => print_progress(format_args!(
            "Added dir: {source_prefix}{path} at sector {sector}\n"
        )),
        ProgressInfo::FileAdded(path, sector) => print_progress(format_args!(
            "Added file: {source_prefix}{path} at sector {sector}\n"
        )),
        _ => {}
    };

    if meta.is_dir() {
        let fs = StdFilesystem::create(source_path);
        compress(fs, media_patch, output, progress)
    } else if meta.is_file() {
        let source = open_image_raw(source_path)?;
        let fs = XDVDFSFilesystem::<_, [u8], DefaultCopier<_, _>>::new(source)?;
        compress(fs, media_patch, output, progress)
    } else {
        bail!("Source must be a directory or an image file");
    }
}

fn compress<F>(
    fs: F,
    media_patch: bool,
    output: &mut SplitOutput,
    progress: impl FnMut(ProgressInfo),
) -> Result<Vec<BufWriter<File>>, anyhow::Error>
where
    F: FilesystemHierarchy + FilesystemCopier<[u8]>,
{
    if media_patch {
        write_cso(MediaPatchFilesystem::new(fs), output, progress)
    } else {
        write_cso(fs, output, progress)
    }
}

fn write_cso<F>(
    fs: F,
    output: &mut SplitOutput,
    progress: impl FnMut(ProgressInfo),
) -> Result<Vec<BufWriter<File>>, anyhow::Error>
where
    F: FilesystemHierarchy + FilesystemCopier<[u8]>,
{
    // Created before packing, so a bad output path fails fast
    let mut first = output.create_part(1)?;

    let mut slbd = SectorLinearBlockDevice::default();
    let mut slbfs = SectorLinearBlockFilesystem::new(fs);
    create_xdvdfs_image(&mut slbfs, &mut slbd, progress)?;

    let len = slbd.size();
    let mut image = SectorLinearImage::new(&slbd, &mut slbfs);
    let second = xdvdfs::cso::write(
        &mut image,
        len,
        FATX_SPLIT_SIZE,
        &mut first,
        || output.create_part(2).map_err(io::Error::other),
        |done, total| print_progress(format_args!("\rCompressing sectors ({done}/{total})")),
    )?;
    print_progress(format_args!("\n"));

    Ok([first].into_iter().chain(second).collect())
}

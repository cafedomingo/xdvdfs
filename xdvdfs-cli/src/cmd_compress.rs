use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use anyhow::bail;
use clap::Args;

use xdvdfs::{
    blockdev,
    write::{self, img::ProgressInfo},
};

use crate::cmd_pack::{check_image_path, default_image_path};
use crate::img::{absolute_path, open_image_raw};

#[derive(Args)]
#[command(about = "Pack and compress an image from a given directory or source ISO image")]
pub struct CompressArgs {
    #[arg(help = "Path to source directory or ISO image")]
    source_path: String,

    #[arg(help = "Path to output image")]
    image_path: Option<String>,
}

struct SplitStdFs;

type BufFile = std::io::BufWriter<std::fs::File>;
type BufFileSectorLinearFs<'a> = write::fs::SectorLinearBlockFilesystem<
    &'a mut write::fs::XDVDFSFilesystem<
        blockdev::OffsetWrapper<std::io::BufReader<std::fs::File>>,
        [u8],
        write::fs::DefaultCopier<blockdev::OffsetWrapper<std::io::BufReader<std::fs::File>>, [u8]>,
    >,
>;

impl ciso::split::SplitFilesystem<std::io::Error, BufFile> for SplitStdFs {
    fn create_file(&mut self, name: &std::ffi::OsStr) -> Result<BufFile, std::io::Error> {
        let file = std::fs::File::create(name)?;
        let bf: BufFile = std::io::BufWriter::new(file);
        Ok(bf)
    }

    fn close(&mut self, _: BufFile) {}
}

pub fn cmd_compress(args: &CompressArgs) -> Result<(), anyhow::Error> {
    let source_path = absolute_path(Path::new(&args.source_path))?;
    let meta = std::fs::metadata(&source_path)?;
    let is_dir = meta.is_dir();

    let image_path = match &args.image_path {
        Some(image_path) => PathBuf::from(image_path),
        None => default_image_path(&source_path, is_dir, "cso")?,
    };
    check_image_path(&source_path, &image_path)?;

    let mut output = ciso::split::SplitOutput::new(SplitStdFs, image_path);

    let source_prefix = if is_dir {
        source_path.to_string_lossy()
    } else {
        Cow::Borrowed("")
    };
    let progress_callback = |pi: ProgressInfo<'_>| match pi {
        ProgressInfo::DirAdded(path, sector) => {
            println!("Added dir: {source_prefix}{path} at sector {sector}");
        }
        ProgressInfo::FileAdded(path, sector) => {
            println!("Added file: {source_prefix}{path} at sector {sector}");
        }
        _ => {}
    };

    let mut total_sectors = 0;
    let mut sectors_finished = 0;
    let progress_callback_compression = |pi| match pi {
        ciso::write::ProgressInfo::SectorCount(c) => total_sectors = c,
        ciso::write::ProgressInfo::SectorFinished => {
            sectors_finished += 1;
            print!("\rCompressing sectors ({sectors_finished}/{total_sectors})");
        }
        ciso::write::ProgressInfo::Finished => println!(),
        _ => {}
    };

    if is_dir {
        let mut fs = write::fs::StdFilesystem::create(&source_path);
        let mut slbd = write::fs::SectorLinearBlockDevice::default();
        let mut slbfs = write::fs::SectorLinearBlockFilesystem::new(&mut fs);

        write::img::create_xdvdfs_image(&mut slbfs, &mut slbd, progress_callback)?;

        let mut input = write::fs::SectorLinearImage::new(&slbd, &mut slbfs);
        ciso::write::write_ciso_image(&mut input, &mut output, progress_callback_compression)?;
    } else if meta.is_file() {
        let source = open_image_raw(&source_path)?;
        let mut fs = write::fs::XDVDFSFilesystem::new(source)?;

        let mut slbd = write::fs::SectorLinearBlockDevice::default();
        let mut slbfs: BufFileSectorLinearFs = write::fs::SectorLinearBlockFilesystem::new(&mut fs);

        write::img::create_xdvdfs_image(&mut slbfs, &mut slbd, progress_callback)?;

        let mut input = write::fs::SectorLinearImage::new(&slbd, &mut slbfs);
        ciso::write::write_ciso_image(&mut input, &mut output, progress_callback_compression)?;
    } else {
        bail!("Source must be a directory or an image file");
    }

    Ok(())
}

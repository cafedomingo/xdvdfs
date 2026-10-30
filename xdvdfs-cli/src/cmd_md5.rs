use crate::img::{open_image, under, under_dir};
use clap::Args;
use md5::{Digest, Md5};
use std::path::Path;
use xdvdfs::blockdev::BlockDeviceRead;
use xdvdfs::write::fs::ImagePath;

#[derive(Args)]
#[command(about = "Show MD5 checksums for files in an image")]
pub struct Md5Args {
    #[arg(help = "Path to XISO image")]
    image_path: String,

    #[arg(help = "Target file within image")]
    path: Option<String>,
}

fn md5_file_dirent<BDR: BlockDeviceRead + ?Sized>(
    img: &mut BDR,
    file: xdvdfs::layout::DirectoryEntryNode,
    path: &ImagePath,
) -> anyhow::Result<String> {
    let mut hasher = Md5::new();
    let dirent = file.node.dirent;
    dirent
        .read_data_chunks(img, |chunk| hasher.update(chunk))
        .map_err(|err| {
            if dirent.is_directory() {
                err.in_directory(path.clone())
            } else {
                err.in_file(path.clone())
            }
        })?;
    let result = hasher.finalize();

    Ok(format!("{result:x}"))
}

fn md5_file_tree<BDR: BlockDeviceRead + ?Sized>(
    img: &mut BDR,
    tree: &[(ImagePath, xdvdfs::layout::DirectoryEntryNode)],
    base: &ImagePath,
) -> anyhow::Result<()> {
    for (dir, file) in tree {
        let path = under(base, dir).join(&file.name_str()?);
        let checksum = md5_file_dirent(img, *file, &path)?;
        println!("{checksum}  {path}");
    }

    Ok(())
}

fn md5_from_file_path<BDR: BlockDeviceRead + ?Sized>(
    volume: &xdvdfs::layout::VolumeDescriptor,
    img: &mut BDR,
    file: &ImagePath,
) -> anyhow::Result<()> {
    let dirent = volume.root_table.walk_path(img, file)?;
    if let Some(table) = dirent.node.dirent.dirent_table() {
        let tree = table.file_tree(img).map_err(under_dir(file))?;
        md5_file_tree(img, &tree, file)?;
    } else {
        let checksum = md5_file_dirent(img, dirent, file)?;
        println!("{checksum}  {file}");
    }

    Ok(())
}

fn md5_from_root_tree<BDR: BlockDeviceRead + ?Sized>(
    volume: &xdvdfs::layout::VolumeDescriptor,
    img: &mut BDR,
) -> anyhow::Result<()> {
    let tree = volume.root_table.file_tree(img)?;
    md5_file_tree(img, &tree, &ImagePath::root())
}

pub fn cmd_md5(args: &Md5Args) -> Result<(), anyhow::Error> {
    let mut img = open_image(Path::new(&args.image_path))?;
    let volume = xdvdfs::read::read_volume(img.as_mut())?;

    if let Some(path) = &args.path {
        md5_from_file_path(&volume, img.as_mut(), &path.as_str().into())
    } else {
        md5_from_root_tree(&volume, img.as_mut())
    }
}

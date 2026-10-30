use crate::img::{open_image_raw, under_dir};
use anyhow::{anyhow, ensure};
use clap::Args;
use std::{
    fs::File,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};
use xdvdfs::{
    blockdev::OffsetWrapper,
    layout::{DirectoryEntryDiskData, DirectoryEntryTable},
    read::DATA_CHUNK_LEN,
    write::fs::ImagePath,
};

fn extract_file(
    img: &mut OffsetWrapper<BufReader<File>>,
    dirent: &DirectoryEntryDiskData,
    path: &Path,
) -> anyhow::Result<()> {
    let mut file = File::create(path)?;
    let size = u64::from(dirent.data.size);
    if size == 0 {
        return Ok(());
    }

    dirent.seek_to(img)?;
    let mut data = img.get_mut().take(size);
    let mut chunk = vec![0; size.min(DATA_CHUNK_LEN as u64) as usize];
    let mut copied = 0;
    while copied < size {
        let read = data.read(&mut chunk)?;
        ensure!(
            read > 0,
            "{} is truncated: the image holds {copied} of its {size} bytes",
            path.display()
        );
        file.write_all(&chunk[..read])?;
        copied += read as u64;
    }

    Ok(())
}

/// Copies out `dirtab`, the table of `dir` in the image
fn copyout_directory(
    img: &mut OffsetWrapper<BufReader<File>>,
    dest_dir: &Path,
    dir: &ImagePath,
    dirtab: &DirectoryEntryTable,
) -> anyhow::Result<()> {
    let tree = dirtab.file_tree(img).map_err(under_dir(dir))?;

    for (dir, dirent) in &tree {
        let dirname = dir.to_os_path(dest_dir);
        let file_name = dirent.name_str()?;
        let file_path = dirname.join(&*file_name);
        let is_dir = dirent.node.dirent.is_directory();

        println!(
            "Extracting {} {}",
            if is_dir { "directory" } else { "file" },
            file_path.display()
        );

        std::fs::create_dir_all(dirname)?;
        if is_dir {
            std::fs::create_dir_all(file_path)?;
        } else {
            extract_file(img, &dirent.node.dirent, &file_path)?;
        }
    }

    Ok(())
}

#[derive(Args)]
#[command(about = "Unpack an entire image to a directory")]
pub struct UnpackArgs {
    #[arg(help = "Path to XISO image")]
    pub image_path: String,

    #[arg(help = "Output directory")]
    pub path: Option<String>,
}

pub fn cmd_unpack(args: &UnpackArgs) -> Result<(), anyhow::Error> {
    let target_dir = match &args.path {
        Some(path) => PathBuf::from_str(path).unwrap(),
        None => {
            let image_path = Path::new(&args.image_path);
            let file_name = image_path.file_name().ok_or_else(|| {
                anyhow!("{image_path:?} has no file name; pass an output directory")
            })?;
            PathBuf::from(file_name).with_extension("")
        }
    };

    let mut img = open_image_raw(Path::new(&args.image_path))?;
    let volume = xdvdfs::read::read_volume(&mut img)?;

    copyout_directory(
        &mut img,
        &target_dir,
        &ImagePath::root(),
        &volume.root_table,
    )
}

#[derive(Args)]
#[command(about = "Copy a file or directory out of the provided image file")]
pub struct CopyOutArgs {
    #[arg(help = "Path to XISO image")]
    image: String,

    #[arg(help = "Path to source file/directory inside image")]
    src_path: String,

    #[arg(help = "Path to destination on the host")]
    dest_path: String,
}

pub fn cmd_copyout(args: &CopyOutArgs) -> Result<(), anyhow::Error> {
    let mut img = open_image_raw(Path::new(&args.image))?;
    let dest_path = Path::new(&args.dest_path);

    let volume = xdvdfs::read::read_volume(&mut img)?;
    let src_path = ImagePath::from(args.src_path.as_str());
    if src_path.is_root() {
        return copyout_directory(&mut img, dest_path, &src_path, &volume.root_table);
    }

    let dirent = volume.root_table.walk_path(&mut img, &src_path)?;
    if let Some(table) = dirent.node.dirent.dirent_table() {
        return copyout_directory(&mut img, dest_path, &src_path, &table);
    }

    extract_file(&mut img, &dirent.node.dirent, dest_path)
}

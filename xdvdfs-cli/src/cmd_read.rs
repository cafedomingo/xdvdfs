use crate::img::open_image;
use clap::Args;
use std::path::Path;
use xdvdfs::write::fs::ImagePath;

#[derive(Args)]
#[command(about = "List files in an image")]
pub struct LsArgs {
    #[arg(help = "Path to XISO image")]
    image_path: String,

    #[arg(default_value = "/", help = "Directory to list")]
    path: String,

    #[arg(
        short = 's',
        long = "scan",
        help = "List using directory scan instead of tree walk"
    )]
    scan: bool,
}

pub fn cmd_ls(args: &LsArgs) -> anyhow::Result<()> {
    let mut img = open_image(Path::new(&args.image_path))?;
    let volume = xdvdfs::read::read_volume(img.as_mut())?;

    let path = ImagePath::from(args.path.as_str());
    let dirent_table = if path.is_root() {
        volume.root_table
    } else {
        volume
            .root_table
            .walk_path(img.as_mut(), &path)?
            .node
            .dirent
            .dirent_table()
            .ok_or(anyhow::anyhow!("Not a directory"))?
    };

    let in_dir = |err: xdvdfs::Error| err.in_directory(path.clone());
    if args.scan {
        let mut iter = dirent_table
            .scan_dirent_tree(img.as_mut())
            .map_err(in_dir)?;

        while let Some(dirent) = iter.next_entry().map_err(in_dir)? {
            let name = dirent.name_str()?;
            println!("{name}");
        }

        return Ok(());
    }

    let listing = dirent_table
        .walk_dirent_tree(img.as_mut())
        .map_err(in_dir)?;

    for dirent in listing {
        let name = dirent.name_str()?;
        println!("{name}");
    }

    Ok(())
}

#[derive(Args)]
#[command(about = "List all files in an image, recursively")]
pub struct TreeArgs {
    #[arg(help = "Path to XISO image")]
    image_path: String,
}

pub fn cmd_tree(args: &TreeArgs) -> anyhow::Result<()> {
    let mut img = open_image(Path::new(&args.image_path))?;
    let volume = xdvdfs::read::read_volume(img.as_mut())?;

    let tree = volume.root_table.file_tree(img.as_mut())?;

    let mut total_size: usize = 0;
    let mut file_count: usize = 0;

    for (dir, file) in &tree {
        let is_dir = file.node.dirent.is_directory();
        let path = dir.join(&file.name_str()?);
        if is_dir {
            println!("{path}/ (0 bytes)");
        } else {
            total_size += file.node.dirent.data.size as usize;
            file_count += 1;

            println!("{} ({} bytes)", path, file.node.dirent.data.size);
        }
    }

    println!(
        "{} files, {} bytes in {}",
        file_count, total_size, args.image_path
    );

    Ok(())
}

#[derive(Args)]
#[command(about = "Compute deterministic checksum of image contents")]
pub struct ChecksumArgs {
    #[arg(help = "Path to XISO image")]
    images: Vec<String>,

    #[arg(short, long, help = "Only output checksums without warnings")]
    silent: bool,
}

fn checksum_single(img_path: &str) -> anyhow::Result<()> {
    let mut img = open_image(Path::new(img_path))?;
    let volume = xdvdfs::read::read_volume(img.as_mut())?;
    let checksum = xdvdfs::checksum::checksum(img.as_mut(), &volume)?;

    for byte in checksum {
        print!("{byte:02x}");
    }

    println!("\t{img_path}");
    Ok(())
}

pub fn cmd_checksum(args: &ChecksumArgs) -> anyhow::Result<()> {
    if !args.silent {
        eprintln!("This SHA256 sum is a condensed checksum of the all the data inside the image");
        eprintln!(
            "It does not encode information about the filesystem structure outside of the data being in the correct order."
        );
        eprintln!(
            "Note that this is NOT a SHA256 sum of the full image, and cannot be compared to a SHA256 sum of the full image."
        );
        eprintln!(
            "This checksum is only useful when compared to other checksums created by this tool."
        );
        eprintln!();
    }

    for image in &args.images {
        checksum_single(image)?;
    }

    Ok(())
}

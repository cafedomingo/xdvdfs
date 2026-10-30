use std::io::{self, Write};
use std::path::Path;

use clap::Args;
use xdvdfs::blockdev::BlockDeviceRead;
use xdvdfs::layout::{DirectoryEntryNode, DirectoryEntryTable, VolumeDescriptor};
use xdvdfs::write::fs::ImagePath;

#[derive(Args)]
#[command(
    about = "Print information about image metadata",
    long_about = "\
    Print information about image metadata. \
    If a file is specified, prints its directory entry. \
    If no file is specified, prints volume metadata."
)]
pub struct InfoArgs {
    #[arg(help = "Path to XISO image")]
    image_path: String,

    #[arg(help = "Path to file/directory within image")]
    file_entry: Option<String>,
}

fn print_volume(out: &mut impl Write, volume: &VolumeDescriptor) -> io::Result<()> {
    let root = volume.root_table.region;
    writeln!(out, "{0: <20} {1}", "Valid:", volume.is_valid())?;
    writeln!(out, "{0: <20} {1}", "Creation time:", volume.filetime)?;
    writeln!(
        out,
        "{0: <20} Sector {1} ({2} bytes)",
        "Root Entry:", root.sector, root.size
    )
}

fn print_dirent(out: &mut impl Write, dirent: &DirectoryEntryNode) -> anyhow::Result<()> {
    let name = dirent.name_str()?;
    writeln!(out, "{0: <20} {1}", "Name:", name)?;
    writeln!(out, "{0: <20} {1}", "Offset:", dirent.offset)?;
    writeln!(
        out,
        "{0: <20} {1}",
        "Left Child Offset:",
        if dirent.node.left_entry_offset != 0 {
            format!("{} bytes", 4 * dirent.node.left_entry_offset)
        } else {
            String::from("None")
        }
    )?;
    writeln!(
        out,
        "{0: <20} {1}",
        "Right Child Offset:",
        if dirent.node.right_entry_offset != 0 {
            format!("{} bytes", 4 * dirent.node.right_entry_offset)
        } else {
            String::from("None")
        }
    )?;

    let data = dirent.node.dirent.data;
    writeln!(
        out,
        "{0: <20} Sector {1} ({2} bytes)",
        "Data Location:", data.sector, data.size
    )?;

    writeln!(
        out,
        "{0: <20} {1}",
        "Attributes:", dirent.node.dirent.attributes
    )?;
    Ok(())
}

fn print_subdir<BDR: BlockDeviceRead + ?Sized>(
    out: &mut impl Write,
    path: &ImagePath,
    subdir: &DirectoryEntryTable,
    img: &mut BDR,
) -> anyhow::Result<()> {
    let children = subdir
        .walk_dirent_tree(img)
        .map_err(|err| err.in_directory(path.clone()))?;
    for node in children {
        let name = node.name_str()?;
        writeln!(out, "{name}")?;
        print_dirent(out, &node)?;
        writeln!(out)?;
    }

    Ok(())
}

pub fn cmd_info(args: &InfoArgs) -> anyhow::Result<()> {
    let image_path = Path::new(&args.image_path);
    let mut out = io::stdout().lock();

    if let Some(path) = &args.file_entry {
        let mut image = crate::img::open_image(image_path)?;
        let volume = xdvdfs::read::read_volume(image.as_mut())?;

        let path = ImagePath::from(path.as_str());
        if path.is_root() {
            print_subdir(&mut out, &path, &volume.root_table, image.as_mut())?;
            return Ok(());
        }

        let dirent = volume.root_table.walk_path(image.as_mut(), &path)?;
        print_dirent(&mut out, &dirent)?;

        if let Some(subdir) = dirent.node.dirent.dirent_table() {
            writeln!(out)?;
            print_subdir(&mut out, &path, &subdir, image.as_mut())?;
        }

        return Ok(());
    }

    let volume = if image_path.extension().is_some_and(|e| e == "cso") {
        let mut image = crate::img::open_image(image_path)?;
        writeln!(out, "{:<20} CISO", "Image type:")?;
        xdvdfs::read::read_volume(image.as_mut())?
    } else {
        let mut image = crate::img::open_image_raw(image_path)?;
        let xgd_type = image.get_offset();

        writeln!(
            out,
            "{:<20} {:?} ({} byte offset)",
            "Image type:",
            xgd_type,
            u64::from(xgd_type)
        )?;

        xdvdfs::read::read_volume(&mut image)?
    };

    print_volume(&mut out, &volume)?;
    Ok(())
}

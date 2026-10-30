use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{ArgAction, Parser, Subcommand};

use crate::img::absolute_path;

#[derive(Subcommand)]
#[command(
    subcommand_help_heading = "Mode",
    subcommand_value_name = "MODE",
    disable_help_subcommand = true
)]
pub enum EXMode {
    #[command(
        short_flag = 'c',
        override_usage = "-c <DIR> [FILE] [-c <DIR> [FILE]]...",
        about = "Create XISO file from <DIR>, at [FILE]. Can be specified multiple times"
    )]
    C {
        dir: String,
        file: Option<String>,

        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        rest: Vec<String>,
    },

    #[command(short_flag = 'l', about = "List files inside given XISO images")]
    L { xiso: Vec<String> },

    #[command(short_flag = 'x', about = "Extract XISO (default mode if none given)")]
    X { xiso: Vec<String> },

    #[command(short_flag = 'r', about = "Rewrite XISO, moving input to <FILE>.old")]
    R { xiso: Vec<String> },

    #[command(external_subcommand)]
    None(Vec<String>),
}

#[derive(Parser)]
#[command(
    disable_version_flag = true,
    version = concat!("(extract-xiso compatibility mode) ", env!("CARGO_PKG_VERSION")),
)]
pub struct EXCommand {
    #[arg(
        short = 'd',
        help = "In extract mode, expand in directory.\nIn rewrite mode, output xiso in directory."
    )]
    directory: Option<String>,

    #[arg(short = 'D', help = "In rewrite mode, delete the original image")]
    delete: bool,

    #[arg(short = 'm', help = "No-op placeholder for compatibility")]
    flag_m: bool,

    #[arg(short = 'q', help = "Silence error output (unimplemented)")]
    quiet_error: bool,

    #[arg(short = 'Q', help = "Silence all output (unimplemented)")]
    quiet_all: bool,

    #[arg(short = 's', help = "No-op placeholder for compatibility")]
    flag_s: bool,

    #[arg(
        short = 'v',
        action = ArgAction::Version,
        help = "Print version",
    )]
    v: (),

    #[command(subcommand)]
    mode: EXMode,
}

fn exiso_create_single(dir: &String, file: &Option<String>) -> anyhow::Result<()> {
    let dir = absolute_path(Path::new(dir))?;
    let meta = std::fs::metadata(&dir)?;
    if !meta.is_dir() {
        bail!("Input is not a directory");
    }

    let image_path = file
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.with_extension("iso"));

    use crate::cmd_pack::*;
    cmd_pack_path(&dir, &image_path)
}

fn exiso_create(dir: &String, file: &Option<String>, rest: &[String]) -> anyhow::Result<()> {
    let mut tasks = Vec::new();
    tasks.push((dir, file.clone()));

    let mut i = 0;
    while i < rest.len() {
        if rest[i] != "-c" {
            bail!(
                "Create arg in position {} must begin with `-c`",
                tasks.len() + 1
            );
        }

        if i + 1 >= rest.len() {
            bail!(
                "Create arg in position {} is missing dir name",
                tasks.len() + 1
            );
        }

        if i + 2 < rest.len() && rest[i + 2] != "-c" {
            tasks.push((&rest[i + 1], Some(rest[i + 2].clone())));
            i += 2;
        } else {
            tasks.push((&rest[i + 1], None));
            i += 2;
        }
    }

    for (dir, file) in tasks {
        exiso_create_single(dir, &file)?;
    }

    Ok(())
}

fn exiso_list(xiso_list: &Vec<String>) -> anyhow::Result<()> {
    use crate::cmd_read::{cmd_tree, TreeArgs};
    for xiso in xiso_list {
        cmd_tree(&TreeArgs {
            image_path: xiso.clone(),
        })?;
    }

    Ok(())
}

fn exiso_extract(xiso_list: &Vec<String>, directory: &Option<String>) -> anyhow::Result<()> {
    use crate::cmd_unpack::{cmd_unpack, UnpackArgs};
    for xiso in xiso_list {
        cmd_unpack(&UnpackArgs {
            image_path: xiso.clone(),
            path: directory.clone(),
        })?;
    }

    Ok(())
}

fn exiso_repack(
    xiso_list: &Vec<String>,
    directory: &Option<String>,
    delete: bool,
) -> anyhow::Result<()> {
    use crate::cmd_pack::pack_into;

    for xiso in xiso_list {
        let source_path = absolute_path(Path::new(&xiso))?;
        let metadata = std::fs::metadata(&source_path)?;
        if !metadata.is_file() {
            bail!("Repack input must be a file");
        }

        let backup_path = source_path.with_added_extension("old");
        if backup_path.exists() {
            bail!("{} already exists", backup_path.display());
        }

        let dest_path = match directory {
            Some(d) => {
                absolute_path(Path::new(d))?.with_file_name(source_path.file_name().unwrap())
            }
            None => source_path.clone(),
        };

        // Packing first leaves the input untouched if it isn't an image
        let partial_path = dest_path.with_added_extension("partial");
        pack_into(&source_path, &partial_path)?;
        if let Err(err) = std::fs::rename(&source_path, &backup_path) {
            let _ = std::fs::remove_file(&partial_path);
            return Err(err).with_context(|| {
                format!("Failed to rename the input to {}", backup_path.display())
            });
        }
        if let Err(err) = std::fs::rename(&partial_path, &dest_path) {
            let _ = std::fs::remove_file(&partial_path);
            let _ = std::fs::rename(&backup_path, &source_path);
            return Err(err).with_context(|| format!("Failed to replace {}", dest_path.display()));
        }

        if delete {
            std::fs::remove_file(&backup_path)?;
        }
    }

    Ok(())
}

pub fn run_exiso_command(cmd: &EXCommand) -> anyhow::Result<()> {
    // FIXME: Respect -q and -Q during processing
    match &cmd.mode {
        EXMode::C { dir, file, rest } => exiso_create(dir, file, rest),
        EXMode::L { xiso } => exiso_list(xiso),
        EXMode::X { xiso } | EXMode::None(xiso) => exiso_extract(xiso, &cmd.directory),
        EXMode::R { xiso } => exiso_repack(xiso, &cmd.directory, cmd.delete),
    }
}

pub fn run_exiso_program(cmd: &EXCommand) -> anyhow::Result<()> {
    let res = run_exiso_command(cmd);

    if !cmd.quiet_error && !cmd.quiet_all {
        res
    } else {
        Ok(())
    }
}

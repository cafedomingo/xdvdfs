use clap::Parser;

mod cmd_compress;
mod cmd_info;
mod cmd_md5;
mod cmd_pack;
mod cmd_read;
mod cmd_unpack;
mod img;
mod progress;

mod exiso_compat;

#[derive(Parser)]
#[command(
    author,
    version,
    about,
    long_about = None,
    arg_required_else_help = true,
    multicall = true,
    allow_external_subcommands = true,
)]
enum TopLevelCommand {
    #[command(name = "xdvdfs", subcommand)]
    Core(Cmd),
    ExtractXiso(exiso_compat::EXCommand),

    // Default any other multicall command to `xdvdfs`
    // external_subcommand must parse into something, even if we don't use it,
    // which necessitates the `dead_code` allow.
    #[command(external_subcommand)]
    #[allow(dead_code)]
    External(Vec<String>),
}

#[derive(Parser)]
#[command(author, version, about, long_about = None, arg_required_else_help = true)]
enum Cmd {
    Ls(cmd_read::LsArgs),
    Tree(cmd_read::TreeArgs),
    Md5(cmd_md5::Md5Args),
    Checksum(cmd_read::ChecksumArgs),
    Info(cmd_info::InfoArgs),
    CopyOut(cmd_unpack::CopyOutArgs),
    Unpack(cmd_unpack::UnpackArgs),
    Pack(cmd_pack::PackArgs),
    Compress(cmd_compress::CompressArgs),

    #[command(hide = true)]
    ExtractXiso(exiso_compat::EXCommand),
}

fn run_command(cmd: &Cmd) -> Result<(), anyhow::Error> {
    use Cmd::*;
    match cmd {
        Ls(args) => cmd_read::cmd_ls(args),
        Tree(args) => cmd_read::cmd_tree(args),
        Md5(args) => cmd_md5::cmd_md5(args),
        Checksum(args) => cmd_read::cmd_checksum(args),
        Info(args) => cmd_info::cmd_info(args),
        CopyOut(args) => cmd_unpack::cmd_copyout(args),
        Unpack(args) => cmd_unpack::cmd_unpack(args),
        Pack(args) => cmd_pack::cmd_pack(args),
        Compress(args) => cmd_compress::cmd_compress(args),
        ExtractXiso(args) => exiso_compat::run_exiso_program(args),
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let tlc = TopLevelCommand::parse();
    match &tlc {
        TopLevelCommand::Core(cli) => run_command(cli),
        TopLevelCommand::ExtractXiso(cmd) => exiso_compat::run_exiso_program(cmd),
        TopLevelCommand::External(_) => run_command(&Cmd::parse()),
    }
}

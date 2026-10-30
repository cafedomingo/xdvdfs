use std::io;

use clap::Parser;

mod cmd_compress;
mod cmd_info;
mod cmd_md5;
mod cmd_pack;
mod cmd_read;
mod cmd_unpack;
mod img;
mod progress;
mod split;

#[derive(Parser)]
#[command(
    name = "xdvdfs",
    author,
    version,
    about,
    long_about = None,
    arg_required_else_help = true
)]
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
}

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cmd = Cmd::parse();
    // Only listings fail on a stdout error; in the other commands a broken
    // pipe comes from the source or output
    let lists = matches!(
        cmd,
        Cmd::Ls(_) | Cmd::Tree(_) | Cmd::Md5(_) | Cmd::Checksum(_) | Cmd::Info(_)
    );
    match run(&cmd) {
        Err(err) if lists && is_broken_pipe(&err) => Ok(()),
        result => result,
    }
}

/// A reader that closes the pipe early, as `head` does, isn't a failure
fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|err| err.kind() == io::ErrorKind::BrokenPipe)
    })
}

fn run(cmd: &Cmd) -> anyhow::Result<()> {
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
    }
}

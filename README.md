# xdvdfs

This is a fork of [antangelo/xdvdfs](https://github.com/antangelo/xdvdfs) focused on the command line tool.

`xdvdfs` is a collection of tools for interacting with XDVDFS/XISO images.

## xdvdfs-cli

`xdvdfs-cli` is a command line tool for interacting with xiso files.

Prebuilt binaries are on the [Releases page](https://github.com/cafedomingo/xdvdfs/releases). To build from source:

```sh
cargo install --git https://github.com/cafedomingo/xdvdfs xdvdfs-cli
```

### Usage

Running `xdvdfs` with no args will bring up the help screen, showing supported subcommands:

```sh
$ xdvdfs
Usage: xdvdfs [COMMAND]

Commands:
  ls           List files in an image
  tree         List all files in an image, recursively
  md5          Show MD5 checksums for files in an image
  checksum     Compute deterministic checksum of image contents
  info         Print information about image metadata
  copy-out     Copy a file or directory out of the provided image file
  unpack       Unpack an entire image to a directory
  pack         Pack an image from a given directory or source ISO image
  compress     Pack and compress an image from a given directory or source ISO image
  help         Print this message or the help of the given subcommand(s)
```

Running a subcommand with the `-h` flag will show help information for that specific subcommand.

If an image is cut short, commands that read it name the file or directory that runs past its end. A read error from a damaged dump is reported as itself, not as "not an xdvdfs volume".

#### Packing an Image

To pack an image from a directory, run:

```sh
xdvdfs pack <directory> [optional output path]
```

This will create an iso that matches 1-to-1 with the input directory.
Symlinks are followed; a directory symlink that loops is an error. If packing fails, the error names the file at fault and any existing image at the output path is left untouched.

#### Repacking an Image

Images can be repacked from an existing ISO image:

```sh
xdvdfs pack <input-image> [optional output path]
```

This will create an iso that matches 1-to-1 with the input image.

#### Unpacking

To unpack an image, run:

```sh
xdvdfs unpack <path to image> [optional output path]
```

Unpacking rejects images with file names that could escape the output directory, such as `..` or names containing `/`, `\` or `:`, and can re-run into a folder that holds an earlier unpack, overwriting its files.

#### Other Utilities

`xdvdfs-cli` supports additional utility tools for use with images.

| Command | Action |
| - | - |
| `xdvdfs ls <path to image> [path within image]` | Lists files within the specified directory, defaulting to root |
| `xdvdfs tree <path to image>` | Prints a listing of every file within the image |
| `xdvdfs md5 <path to image> [optional path to file within image]` | Prints md5 sums for specified files, or every file, within the image |
| `xdvdfs checksum [path to img1]...` | Computes a checksum for all image contents to check integrity against other images |
| `xdvdfs info <path to image> [path within image]` | Prints metadata info for the specified directory entry, or root volume |
| `xdvdfs copy-out <path to image> <path within image> <destination path>` | Copies a single file or directory out of the provided image |

xdvdfs-cli can be run in an extract-xiso (best-effort) compatibility mode by renaming (or symlinking) the executable
to be named `extract-xiso`. The command-line interface then mimics extract-xiso's parameters. Only basic use cases
(i.e. `-xlrc` modes and `-dD` options) are supported, and most options are no-ops.

## xdvdfs-core

`xdvdfs-core` is a library for working with XDVDFS metadata.

A simple example reading a file from a given path is:

```rust
fn read_from_path(xiso: &Path, file_path: &str) -> Vec<u8> {
    let mut xiso = std::fs::File::open(xiso).unwrap();
    let volume = xdvdfs::read::read_volume(&mut xiso).unwrap();

    let file_dirent = volume.root_table.walk_path(&mut xiso, file_path).unwrap();

    let mut data = Vec::new();
    file_dirent
        .node
        .dirent
        .read_data_chunks(&mut xiso, |chunk| data.extend_from_slice(chunk))
        .unwrap();
    data
}
```

Custom block devices can be defined by implementing the traits in `xdvdfs::blockdev`, which report failures as `std::io::Error`. Library functions return `xdvdfs::Result`.

The source code for xdvdfs-cli provides a more detailed example of how to use xdvdfs-core.

Note that xdvdfs is currently not API stable, and following semver with major version 0, each minor version bump may or
may not include breaking changes.

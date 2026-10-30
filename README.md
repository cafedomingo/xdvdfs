# xdvdfs

This is a fork of [antangelo/xdvdfs](https://github.com/antangelo/xdvdfs) focused on the command line tool.

`xdvdfs` is a collection of tools for interacting with XDVDFS/XISO images.

## xdvdfs-cli

`xdvdfs-cli` is a command line tool for interacting with xiso files.

Prebuilt binaries are on the [Releases page](https://github.com/cafedomingo/xdvdfs/releases). To build from source:

```sh
cargo install --git https://github.com/cafedomingo/xdvdfs xdvdfs-cli
```

Building from source needs Rust 1.99 or later.

### Usage

Running `xdvdfs` with no args will bring up the help screen, showing supported subcommands:

```sh
$ xdvdfs
Tool for interacting with XISO/XDVDFS images

Usage: xdvdfs <COMMAND>

Commands:
  ls        List files in an image
  tree      List all files in an image, recursively
  md5       Show MD5 checksums for files in an image
  checksum  Compute deterministic checksum of image contents
  info      Print information about image metadata
  copy-out  Copy a file or directory out of the provided image file
  unpack    Unpack an entire image to a directory
  pack      Pack an image from a given directory or source ISO image
  compress  Pack and compress an image from a given directory or source ISO image
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

Running a subcommand with the `-h` flag will show help information for that specific subcommand.

If an image is cut short, commands that read it name the file or directory that runs past its end. A read error from a damaged dump is reported as itself, not as "not an xdvdfs volume".

#### Packing an Image

To pack an image from a directory, run:

```sh
xdvdfs pack <directory> [optional output path]
```

This will create an iso that matches 1-to-1 with the input directory.
`pack` and `compress` skip the `.DS_Store` files macOS adds to folders.
The image depends only on the names and contents of the files, so packing them again, or repacking the image, gives the same bytes.
`--media-patch` on `pack` or `compress` patches the media check in every `.xbe`, as extract-xiso does by default; without it, files are copied unchanged.
`--skip-system-update` on `pack`, `compress` or `unpack` leaves out every `$SystemUpdate` folder, the dashboard update retail discs carry.
File names are stored in Windows-1252, one byte per character, so a name can be up to 255 characters and can't use characters outside it, such as those in `日本.txt`.
Symlinks are followed; a directory symlink that loops is an error. If packing fails, the error names the file at fault and any existing image at the output path is left untouched.
Neither `pack` nor `compress` creates a missing output folder; the error names the output path.

#### Repacking an Image

Images can be repacked from an existing ISO image:

```sh
xdvdfs pack <input-image> [optional output path]
```

This will create an iso that matches 1-to-1 with the input image.

#### Compressing an Image

To pack a directory or image into a CSO, the compressed format that Project Stellar loads, run:

```sh
xdvdfs compress <directory or input-image> [optional output path]
```

For an output path of `game.cso`, `game` or `game.iso` this writes `game.1.cso`, plus `game.2.cso` once the compressed output passes FATX's 4 GiB file limit, laid out as MakeMHz's [`ciso.py`](https://github.com/MakeMHz/stellar-cso) writes them. A `game.2.cso` left from an earlier, larger image is removed. As with `pack`, a failure leaves any existing output untouched.

`ls`, `tree`, `checksum`, `md5` and `info` read CSOs: `game.1.cso` with any parts after it, or a single-file `game.cso`. `pack`, `compress`, `unpack` and `copy-out` take XISO images only. Split CSOs written by upstream xdvdfs don't read.

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

`xdvdfs tree game.iso | head` exits 0 quietly, as do `ls`, `md5`, `checksum` and `info`.

## xdvdfs-core

`xdvdfs-core` is a library for working with XDVDFS metadata.

A simple example reading a file from a given path is:

```rust
fn read_from_path(xiso: &Path, file_path: &str) -> Vec<u8> {
    let mut xiso = std::fs::File::open(xiso).unwrap();
    let volume = xdvdfs::read::read_volume(&mut xiso).unwrap();

    let file_dirent = volume.root_table.walk_path(&mut xiso, &file_path.into()).unwrap();

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

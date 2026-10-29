mod common;

use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use common::{assert_success, assert_trees_equal, place_at_offset, write_tree, xdvdfs};
use tempfile::tempdir;
use xdvdfs::blockdev::XDVDFSOffsets;

// 0xe header bytes plus the name make 32-byte dirents, so table sizes
// land on sector and offset boundaries.
const NAME_LEN: usize = 18;
const DIRENT_LEN: usize = 0xe + NAME_LEN;
const SECTOR_LEN: usize = 2048;
const MAX_DIRENT_OFFSET: usize = 65536;

fn numbered(i: usize) -> String {
    format!("{i:0NAME_LEN$}")
}

fn run(args: &[&OsStr]) {
    assert_success(&xdvdfs(args));
}

fn pack(src: &Path, img: &Path) {
    run(&["pack".as_ref(), src.as_ref(), img.as_ref()]);
}

fn unpack(img: &Path, out: &Path) {
    run(&["unpack".as_ref(), img.as_ref(), out.as_ref()]);
}

fn assert_unpacks_to(img: &Path, expected: &Path) {
    let out = tempdir().unwrap();
    unpack(img, out.path());
    assert_trees_equal(expected, out.path());
}

/// Packs `src` to `img`, then repacks `img` and checks both unpack to `src`.
fn pack_roundtrip(src: &Path, img: &Path) {
    pack(src, img);
    assert_unpacks_to(img, src);

    let repacked = tempdir().unwrap();
    let repacked_img = repacked.path().join("repacked.xiso");
    pack(img, &repacked_img);
    assert_unpacks_to(&repacked_img, src);
}

fn pack_case(files: &[(&str, &[u8])], empty_dirs: &[&str]) {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    write_tree(&src, files, empty_dirs);
    pack_roundtrip(&src, &work.path().join("img.xiso"));
}

#[test]
fn extract_xiso_compat() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[(&numbered(0), b"extract-xiso compat")], &[]);

    run(&[
        "extract-xiso".as_ref(),
        "-c".as_ref(),
        src.as_ref(),
        img.as_ref(),
    ]);

    let out = tempdir().unwrap();
    run(&[
        "extract-xiso".as_ref(),
        "-d".as_ref(),
        out.path().as_ref(),
        "-x".as_ref(),
        img.as_ref(),
    ]);
    assert_trees_equal(&src, out.path());

    let out = tempdir().unwrap();
    run(&[
        "extract-xiso".as_ref(),
        "-d".as_ref(),
        out.path().as_ref(),
        img.as_ref(),
    ]);
    assert_trees_equal(&src, out.path());

    run(&[
        "extract-xiso".as_ref(),
        "-D".as_ref(),
        "-r".as_ref(),
        img.as_ref(),
    ]);
    let out = tempdir().unwrap();
    run(&[
        "extract-xiso".as_ref(),
        "-d".as_ref(),
        out.path().as_ref(),
        "-x".as_ref(),
        img.as_ref(),
    ]);
    assert_trees_equal(&src, out.path());
}

#[test]
fn empty_file() {
    pack_case(&[("empty_file", b"")], &[]);
}

#[test]
fn empty_root() {
    pack_case(&[], &[]);
}

#[test]
fn empty_subdir() {
    pack_case(&[], &["subdir"]);
}

#[test]
fn special_chars_in_name() {
    // Ü and ü collide on case-insensitive filesystems
    pack_case(
        &[
            ("Ü", "Ü".as_bytes()),
            ("b", b"b"),
            ("ü", "ü".as_bytes()),
            ("á", "á".as_bytes()),
        ],
        &[],
    );
}

#[test]
fn many_files() {
    let names: Vec<_> = (0..MAX_DIRENT_OFFSET / DIRENT_LEN + 1)
        .map(|i| format!("a/{}", numbered(i)))
        .collect();
    let files: Vec<_> = names.iter().map(|n| (n.as_str(), &b"data"[..])).collect();
    pack_case(&files, &[]);
}

#[test]
fn dirent_size_2048() {
    let names: Vec<_> = (0..SECTOR_LEN / DIRENT_LEN)
        .map(|i| format!("a/{}", numbered(i)))
        .collect();
    let mut files: Vec<_> = names.iter().map(|n| (n.as_str(), &b"data"[..])).collect();
    files.push(("b", b"data"));
    pack_case(&files, &[]);
}

#[test]
fn copy_out() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(
        &src,
        &[
            ("b", b"root_b"),
            ("a/f1", b"root_a_f1"),
            ("a/f2", b"root_a_f2"),
        ],
        &[],
    );
    pack(&src, &img);

    let out = tempdir().unwrap();
    for name in ["b", "a"] {
        run(&[
            "copy-out".as_ref(),
            img.as_ref(),
            format!("/{name}").as_ref(),
            out.path().join(name).as_ref(),
        ]);
    }
    assert_trees_equal(&src, out.path());
}

#[test]
fn windows_1252_name_roundtrip() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("café.txt", b"coffee")], &[]);
    pack_roundtrip(&src, &img);

    let ls = xdvdfs(&["ls".as_ref(), img.as_ref()]);
    assert_success(&ls);
    let listing = String::from_utf8(ls.stdout).unwrap();
    assert!(listing.contains("café.txt"), "ls printed:\n{listing}");
}

#[test]
fn xgd2_offset_image() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(
        &src,
        &[("default.xbe", &[1; 5000]), ("a/f1", &[2; 100])],
        &["empty"],
    );
    pack(&src, &img);

    // Closes the write handle; Windows restricts copying a file open for writing
    let wrapped = place_at_offset(&img, u64::from(XDVDFSOffsets::XGD2)).into_temp_path();
    assert_unpacks_to(&wrapped, &src);

    let copy = work.path().join("copy.xiso");
    fs::copy(&wrapped, &copy).unwrap();
    run(&[
        "extract-xiso".as_ref(),
        "-D".as_ref(),
        "-r".as_ref(),
        copy.as_ref(),
    ]);
    assert_unpacks_to(&copy, &src);
}

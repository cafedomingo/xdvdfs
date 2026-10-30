mod common;

use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::Path;

use common::{
    assert_error, assert_success, assert_trees_equal, place_at_offset, write_tree, xdvdfs,
};
use tempfile::tempdir;
use xdvdfs::blockdev::XDVDFSOffsets;
use xdvdfs::layout::DirectoryEntryDiskNode;

// 0xe header bytes plus the name make 32-byte dirents, so table sizes
// land on sector and offset boundaries.
const NAME_LEN: usize = 18;
const DIRENT_HEADER_LEN: usize = 0xe;
const DIRENT_LEN: usize = DIRENT_HEADER_LEN + NAME_LEN;
const SECTOR_LEN: usize = 2048;
const MAX_DIRENT_OFFSET: usize = 65536;
// Mirrors cmd_unpack.rs, which tests can't import from the binary
const EXTRACT_CHUNK_LEN: usize = 1 << 20;

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

fn find_unique(haystack: &[u8], needle: &[u8]) -> usize {
    let mut hits = haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(at, _)| at);
    let at = hits.next().expect("needle not found");
    assert_eq!(hits.next(), None, "needle found more than once");
    at
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
    let multi_chunk = vec![3; 2 * EXTRACT_CHUNK_LEN + 17];
    write_tree(
        &src,
        &[
            ("default.xbe", &[1; 5000]),
            ("a/f1", &[2; 100]),
            ("multi_chunk", &multi_chunk),
        ],
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

#[test]
fn unpack_rejects_path_escape() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("xx_escape", b"data")], &[]);
    pack(&src, &img);

    let mut bytes = fs::read(&img).unwrap();
    let escape = b"../escape";
    let name_at = find_unique(&bytes, b"xx_escape");
    bytes[name_at..][..escape.len()].copy_from_slice(escape);
    fs::write(&img, bytes).unwrap();

    let out = work.path().join("out");
    assert_error(&xdvdfs(&["unpack".as_ref(), img.as_ref(), out.as_ref()]));
    assert!(!work.path().join("escape").exists());
}

#[test]
fn unpack_twice_into_same_dir() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("sub/f", b"data")], &[]);
    pack(&src, &img);

    let out = work.path().join("out");
    unpack(&img, &out);
    unpack(&img, &out);
    assert_trees_equal(&src, &out);
}

#[test]
fn unpack_truncated_image_fails() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("big.bin", &vec![1; 1_000_000])], &[]);
    pack(&src, &img);
    File::options()
        .write(true)
        .open(&img)
        .unwrap()
        .set_len(600_000)
        .unwrap();

    let out = tempdir().unwrap();
    let unpack = xdvdfs(&["unpack".as_ref(), img.as_ref(), out.path().as_ref()]);
    let stderr = assert_error(&unpack);
    assert!(stderr.contains("big.bin"), "unpack printed:\n{stderr}");

    let copy_out = xdvdfs(&[
        "copy-out".as_ref(),
        img.as_ref(),
        "/big.bin".as_ref(),
        out.path().join("big.bin").as_ref(),
    ]);
    let stderr = assert_error(&copy_out);
    assert!(stderr.contains("big.bin"), "copy-out printed:\n{stderr}");
}

#[test]
fn unpack_dot_errors() {
    assert_error(&xdvdfs(&["unpack".as_ref(), ".".as_ref()]));
}

#[test]
fn pack_rejects_image_directory_loop() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("loop_dir/f", b"data")], &[]);
    pack(&src, &img);

    // The root table holds only `loop_dir`, so the sector holding its entry
    // is the root's.
    let mut bytes = fs::read(&img).unwrap();
    let header_at = find_unique(&bytes, b"loop_dir") - DIRENT_HEADER_LEN;
    let header = bytes[header_at..][..DIRENT_HEADER_LEN].try_into().unwrap();
    let mut node = DirectoryEntryDiskNode::deserialize(header).unwrap();
    node.dirent.data.sector = (header_at / SECTOR_LEN) as u32;
    node.serialize_into(&mut bytes[header_at..]).unwrap();
    fs::write(&img, bytes).unwrap();

    let out = work.path().join("out.xiso");
    assert_error(&xdvdfs(&["pack".as_ref(), img.as_ref(), out.as_ref()]));
}

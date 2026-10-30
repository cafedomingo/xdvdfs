mod common;

use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use common::{
    assert_error, assert_success, assert_trees_equal, place_at_offset, write_tree, xdvdfs,
    xdvdfs_in,
};
use md5::{Digest, Md5};
use tempfile::tempdir;
use xdvdfs::blockdev::XDVDFSOffsets;
use xdvdfs::layout::DirectoryEntryDiskNode;
use xdvdfs::read::DATA_CHUNK_LEN;

// 0xe header bytes plus the name make 32-byte dirents, so table sizes
// land on sector and offset boundaries.
const NAME_LEN: usize = 18;
const DIRENT_HEADER_LEN: usize = 0xe;
const DIRENT_LEN: usize = DIRENT_HEADER_LEN + NAME_LEN;
const SECTOR_LEN: usize = 2048;
const MAX_DIRENT_OFFSET: usize = 65536;
#[cfg(unix)]
const OVERSIZED_FILE_LEN: u64 = (1 << 32) + 1;

const CSO_SIZE_FIELD: usize = 8;
const CSO_HEADER_LEN: usize = 24;
const CSO_COMPRESSED: u32 = 1 << 31;
const CSO_ALIGNMENT: u32 = 2;
/// `xdvdfs checksum` of `tests/fixtures/ciso-0.3.0.cso` from xdvdfs 6cd1a0e,
/// which read it with ciso 0.3.0
const CISO_FIXTURE_CHECKSUM: &str =
    "30b8ec9b15eb469578d0220ff18714b96f5c41327c1ca29c0a313df580a8a014";

fn numbered(i: usize) -> String {
    format!("{i:0NAME_LEN$}")
}

fn run(args: &[&OsStr]) {
    assert_success(&xdvdfs(args));
}

fn pack(src: &Path, img: &Path) {
    run(&["pack".as_ref(), src.as_ref(), img.as_ref()]);
}

fn compress(src: &Path, img: &Path) {
    run(&["compress".as_ref(), src.as_ref(), img.as_ref()]);
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
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

/// Runs a `pack` of `src` that should fail, checks it left no output image
/// beside `src`, and returns its stderr.
fn pack_error(src: &Path) -> String {
    let img = src.with_file_name("out.xiso");
    let stderr = assert_error(&xdvdfs(&["pack".as_ref(), src.as_ref(), img.as_ref()]));
    for path in [&img, &img.with_added_extension("partial")] {
        assert!(!path.exists(), "{} was left behind", path.display());
    }
    stderr
}

fn checksum(img: &Path) -> String {
    let out = xdvdfs(&["checksum".as_ref(), "-s".as_ref(), img.as_ref()]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).unwrap();
    stdout.split('\t').next().unwrap().to_owned()
}

/// Packs a 1,000,000-byte `big.bin` into `work/img.xiso`, then cuts the
/// image to 600,000 bytes.
fn truncated_image(work: &Path) -> PathBuf {
    truncated_image_with(work, &[("big.bin", &vec![1; 1_000_000])], 600_000)
}

/// Where `sub_after_big_image` puts `/sub`'s table
const SUB_TABLE_SECTOR: u64 = 524;

/// Packs `a.txt`, `big.bin` and `sub/f` into `work/img.xiso`, then cuts the
/// image to `len` bytes. A three-entry table is balanced whatever the listing
/// order, so `big.bin` is allocated first, then `a.txt`, then `/sub`'s table.
fn sub_after_big_image(work: &Path, len: u64) -> PathBuf {
    let big = vec![1; 1_000_000];
    let files: &[(&str, &[u8])] = &[("a.txt", b"a"), ("big.bin", &big), ("sub/f", b"f")];
    truncated_image_with(work, files, len)
}

/// Packs `files` into `work/img.xiso`, then cuts the image to `len` bytes.
fn truncated_image_with(work: &Path, files: &[(&str, &[u8])], len: u64) -> PathBuf {
    let src = work.join("src");
    let img = work.join("img.xiso");
    write_tree(&src, files, &[]);
    pack(&src, &img);
    File::options()
        .write(true)
        .open(&img)
        .unwrap()
        .set_len(len)
        .unwrap();
    img
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
fn md5_file_larger_than_one_chunk() {
    let big: Vec<u8> = (0..2 * DATA_CHUNK_LEN + 13)
        .map(|i| (i % 251) as u8)
        .collect();
    let files: [(&str, &[u8]); 2] = [("big.bin", &big), ("small.txt", b"small")];
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &files, &[]);
    pack(&src, &img);

    let out = xdvdfs(&["md5".as_ref(), img.as_ref()]);
    assert_success(&out);
    // md5 prints in image order, which follows the host's directory listing.
    let mut actual: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    actual.sort();
    let mut expected: Vec<String> = files
        .iter()
        .map(|(name, data)| format!("{:x}  /{name}", Md5::digest(data)))
        .collect();
    expected.sort();
    assert_eq!(actual, expected);
}

#[test]
fn md5_subdirectory_paths() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("sub/g", b"g"), ("sub/x/f", b"f")], &[]);
    pack(&src, &img);

    let out = xdvdfs(&["md5".as_ref(), img.as_ref(), "/sub".as_ref()]);
    assert_success(&out);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let mut paths: Vec<&str> = stdout
        .lines()
        .map(|line| line.split_once("  ").unwrap().1)
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        ["/sub/g", "/sub/x", "/sub/x/f"],
        "md5 printed:\n{stdout}"
    );

    let nested = format!("{:x}  /sub/x/f", Md5::digest(b"f"));
    assert!(
        stdout.lines().any(|line| line == nested),
        "md5 printed:\n{stdout}"
    );
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
fn pack_name_beyond_ascii() {
    // `Œ` is U+0152, past a byte, though Windows-1252 encodes it
    pack_case(
        &[("x", b"x"), ("Œuvre.txt", b"oeuvre"), ("é.txt", b"e")],
        &[],
    );
}

#[test]
fn xgd2_offset_image() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    let multi_chunk = vec![3; 2 * DATA_CHUNK_LEN + 17];
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
    let img = truncated_image(work.path());

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
fn truncated_file_names_file() {
    let work = tempdir().unwrap();
    let img = truncated_image(work.path());

    for cmd in ["checksum", "md5"] {
        let stderr = assert_error(&xdvdfs(&[cmd.as_ref(), img.as_ref()]));
        assert!(
            stderr.contains("/big.bin: file runs past the end"),
            "{cmd} printed:\n{stderr}"
        );
    }
}

#[test]
fn truncated_directory_table_names_directory() {
    let work = tempdir().unwrap();
    let img = sub_after_big_image(work.path(), 600_000);
    let out = work.path().join("out");
    let truncated = "/sub: directory table runs past the end";

    let stderr = pack_error(&img);
    assert!(stderr.contains(truncated), "pack printed:\n{stderr}");

    let commands: &[&[&OsStr]] = &[
        &["compress".as_ref(), img.as_ref()],
        &["unpack".as_ref(), img.as_ref(), out.as_ref()],
        &[
            "copy-out".as_ref(),
            img.as_ref(),
            "/sub/f".as_ref(),
            out.as_ref(),
        ],
        &[
            "copy-out".as_ref(),
            img.as_ref(),
            "/sub".as_ref(),
            out.as_ref(),
        ],
        &["tree".as_ref(), img.as_ref()],
        &["checksum".as_ref(), img.as_ref()],
        &["md5".as_ref(), img.as_ref()],
        &["md5".as_ref(), img.as_ref(), "/sub".as_ref()],
        &["ls".as_ref(), img.as_ref(), "/sub".as_ref()],
        &["info".as_ref(), img.as_ref(), "/sub".as_ref()],
    ];
    for args in commands {
        // compress's default output goes in the working directory
        let stderr = assert_error(&xdvdfs_in(work.path(), args));
        assert!(stderr.contains(truncated), "{args:?} printed:\n{stderr}");
    }
}

#[test]
fn md5_names_directory_cut_inside_its_table() {
    let work = tempdir().unwrap();
    // Keeps `/sub`'s one entry, which md5 lists, but not the rest of the
    // table's sector, which md5 hashes
    let img = sub_after_big_image(work.path(), SUB_TABLE_SECTOR * 2048 + 100);
    let stderr = assert_error(&xdvdfs(&["md5".as_ref(), img.as_ref()]));
    assert!(
        stderr.contains("/sub: directory table runs past the end"),
        "md5 printed:\n{stderr}"
    );
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

#[test]
fn pack_unencodable_name_names_file() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    write_tree(&src, &[("日本.txt", b"data")], &[]);

    let stderr = pack_error(&src);
    assert!(
        stderr.contains("日本.txt") && stderr.contains("rename"),
        "pack printed:\n{stderr}"
    );
}

#[test]
fn pack_decomposed_name_names_file() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let name = "cafe\u{301}.txt";
    write_tree(&src, &[(name, b"coffee")], &[]);

    let stderr = pack_error(&src);
    assert!(
        stderr.contains(name) && stderr.contains("rename"),
        "pack printed:\n{stderr}"
    );
}

#[test]
fn pack_failure_keeps_existing_image() {
    let work = tempdir().unwrap();
    let good = work.path().join("good");
    let bad = work.path().join("bad");
    let img = work.path().join("out.iso");
    write_tree(&good, &[("f", b"good")], &[]);
    write_tree(&bad, &[("日本.txt", b"bad")], &[]);
    pack(&good, &img);
    let packed = fs::read(&img).unwrap();

    assert_error(&xdvdfs(&["pack".as_ref(), bad.as_ref(), img.as_ref()]));
    assert!(fs::read(&img).unwrap() == packed, "out.iso changed");
}

#[cfg(unix)]
#[test]
fn pack_oversized_file_names_file() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    fs::create_dir(&src).unwrap();
    File::create(src.join("big.bin"))
        .unwrap()
        .set_len(OVERSIZED_FILE_LEN)
        .unwrap();

    let stderr = pack_error(&src);
    assert!(stderr.contains("big.bin"), "pack printed:\n{stderr}");
}

#[cfg(unix)]
#[test]
fn pack_socket_names_file() {
    use std::os::unix::net::UnixListener;

    let work = tempdir().unwrap();
    let src = work.path().join("src");
    fs::create_dir(&src).unwrap();
    let _listener = UnixListener::bind(src.join("sock")).unwrap();

    let stderr = pack_error(&src);
    assert!(stderr.contains("src/sock"), "pack printed:\n{stderr}");
}

#[test]
fn pack_truncated_image_source_fails() {
    let work = tempdir().unwrap();
    let img = truncated_image(work.path());

    let stderr = pack_error(&img);
    assert!(stderr.contains("big.bin"), "pack printed:\n{stderr}");
}

#[test]
fn compress_truncated_image_source_fails() {
    let work = tempdir().unwrap();
    let img = truncated_image(work.path());

    // compress's default output goes in the working directory
    let compress = xdvdfs_in(work.path(), &["compress".as_ref(), img.as_ref()]);
    let stderr = assert_error(&compress);
    assert!(stderr.contains("big.bin"), "compress printed:\n{stderr}");
}

#[test]
fn compress_dot_source() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    write_tree(&src, &[("f", b"data")], &[]);

    for cmd in ["pack", "compress"] {
        let stderr = assert_error(&xdvdfs_in(&src, &[cmd.as_ref(), ".".as_ref()]));
        assert!(
            stderr.contains("Destination path is contained by source path"),
            "{cmd} printed:\n{stderr}"
        );
    }

    assert_success(&xdvdfs_in(
        &src,
        &["pack".as_ref(), ".".as_ref(), "../out.iso".as_ref()],
    ));
    assert_unpacks_to(&work.path().join("out.iso"), &src);
    assert_success(&xdvdfs_in(
        &src,
        &["compress".as_ref(), ".".as_ref(), "../out.cso".as_ref()],
    ));
    assert!(work.path().join("out.1.cso").exists());
}

#[test]
fn compress_matches_pack() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(
        &src,
        &[
            ("unaligned", &[7; SECTOR_LEN + 100]),
            ("empty", b""),
            ("a/small", b"small"),
        ],
        &[],
    );
    pack(&src, &img);

    compress(&src, &work.path().join("out.cso"));
    assert_eq!(checksum(&work.path().join("out.1.cso")), checksum(&img));
}

#[test]
fn compress_output_dir() {
    let work = tempdir().unwrap();
    let cwd = tempdir().unwrap();
    let src = work.path().join("src");
    let sub = work.path().join("sub");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("f", b"data")], &[]);
    fs::create_dir(&sub).unwrap();
    pack(&src, &img);

    assert_success(&xdvdfs_in(
        cwd.path(),
        &[
            "compress".as_ref(),
            src.as_ref(),
            sub.join("out.cso").as_ref(),
        ],
    ));
    assert_eq!(fs::read_dir(cwd.path()).unwrap().count(), 0);
    assert_eq!(names_in(&sub), ["out.1.cso"]);
    assert_eq!(checksum(&sub.join("out.1.cso")), checksum(&img));
}

#[test]
fn compress_removes_stale_part() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("img.xiso");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &img);
    fs::write(work.path().join("out.2.cso"), b"stale").unwrap();

    compress(&src, &work.path().join("out.cso"));
    assert!(!work.path().join("out.2.cso").exists());
    assert_eq!(checksum(&work.path().join("out.1.cso")), checksum(&img));
}

#[test]
fn compress_keeps_source_named_like_a_part() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("out.2.cso");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &img);
    let packed = fs::read(&img).unwrap();

    let out = work.path().join("out.cso");
    assert_error(&xdvdfs(&["compress".as_ref(), img.as_ref(), out.as_ref()]));
    assert!(fs::read(&img).unwrap() == packed, "out.2.cso changed");
    assert_eq!(names_in(work.path()), ["out.2.cso", "src"]);
}

#[test]
fn compress_failure_keeps_existing_output() {
    let work = tempdir().unwrap();
    let good = work.path().join("good");
    let bad = work.path().join("bad");
    let out = work.path().join("out.cso");
    write_tree(&good, &[("f", b"good")], &[]);
    write_tree(&bad, &[("日本.txt", b"bad")], &[]);
    compress(&good, &out);
    let compressed = fs::read(work.path().join("out.1.cso")).unwrap();

    assert_error(&xdvdfs(&["compress".as_ref(), bad.as_ref(), out.as_ref()]));
    assert!(
        fs::read(work.path().join("out.1.cso")).unwrap() == compressed,
        "out.1.cso changed"
    );
    assert_eq!(names_in(work.path()), ["bad", "good", "out.1.cso"]);
}

#[test]
fn cso_reads_ciso_fixture() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ciso-0.3.0.cso");
    let cso = fs::read(&fixture).unwrap();
    let u32_at = |at: usize| u32::from_le_bytes(cso[at..][..4].try_into().unwrap());
    let image_len = u64::from_le_bytes(cso[CSO_SIZE_FIELD..][..8].try_into().unwrap());
    let blocks = (image_len / SECTOR_LEN as u64) as usize;
    let entry = |i: usize| u32_at(CSO_HEADER_LEN + 4 * i);
    let offset = |entry: u32| ((entry & !CSO_COMPRESSED) as usize) << CSO_ALIGNMENT;

    assert!(
        (0..blocks).any(|i| entry(i) & CSO_COMPRESSED == 0),
        "no stored block"
    );
    let last = entry(blocks - 1);
    assert!(last & CSO_COMPRESSED != 0, "last block is stored");
    let last_end = offset(last) + 4 + u32_at(offset(last)) as usize;
    let end_entry = offset(entry(blocks));
    assert!(
        end_entry < last_end && end_entry < cso.len(),
        "final entry {end_entry} isn't floored below {last_end}"
    );

    assert_eq!(checksum(&fixture), CISO_FIXTURE_CHECKSUM);
}

#[test]
fn exiso_r_rejects_non_image() {
    let work = tempdir().unwrap();
    let file = work.path().join("notimage.bin");
    let contents = b"not an xiso image";
    fs::write(&file, contents).unwrap();

    assert_error(&xdvdfs(&[
        "extract-xiso".as_ref(),
        "-r".as_ref(),
        file.as_ref(),
    ]));
    assert_eq!(fs::read(&file).unwrap(), contents);
    for left in ["notimage.bin.old", "notimage.old", "notimage.bin.partial"] {
        assert!(!work.path().join(left).exists(), "{left} was created");
    }
}

#[test]
fn exiso_r_keeps_existing_old() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("g.iso");
    let old = work.path().join("g.iso.old");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &img);
    let packed = fs::read(&img).unwrap();
    fs::write(&old, b"earlier backup").unwrap();

    assert_error(&xdvdfs(&[
        "extract-xiso".as_ref(),
        "-r".as_ref(),
        img.as_ref(),
    ]));
    assert!(fs::read(&img).unwrap() == packed, "g.iso changed");
    assert_eq!(fs::read(&old).unwrap(), b"earlier backup");
}

#[test]
fn exiso_r_backup_name() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("g.iso");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &img);
    let packed = fs::read(&img).unwrap();

    run(&["extract-xiso".as_ref(), "-r".as_ref(), img.as_ref()]);
    let old = work.path().join("g.iso.old");
    assert!(
        fs::read(&old).unwrap() == packed,
        "g.iso.old isn't the input"
    );
    assert!(!work.path().join("g.old").exists());
    assert_unpacks_to(&img, &src);
}

#[test]
fn pack_source_named_like_partial() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let source = work.path().join("g.iso.partial");
    let img = work.path().join("g.iso");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &source);
    let packed = fs::read(&source).unwrap();

    assert_error(&xdvdfs(&["pack".as_ref(), source.as_ref(), img.as_ref()]));
    assert!(
        fs::read(&source).unwrap() == packed,
        "g.iso.partial changed"
    );
    assert!(!img.exists());
}

#[test]
fn pack_output_is_directory() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let out = work.path().join("outdir");
    write_tree(&src, &[("f", b"data")], &[]);
    fs::create_dir(&out).unwrap();

    let stderr = assert_error(&xdvdfs(&["pack".as_ref(), src.as_ref(), out.as_ref()]));
    assert!(stderr.contains("outdir"), "pack printed:\n{stderr}");
    assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    assert!(!out.with_added_extension("partial").exists());
}

#[cfg(unix)]
#[test]
fn pack_through_output_symlink() {
    use std::os::unix::fs::symlink;

    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let store = work.path().join("store");
    write_tree(&src, &[("f", b"data")], &[]);
    fs::create_dir(&store).unwrap();
    fs::write(store.join("old.iso"), b"earlier image").unwrap();

    // An existing target is replaced, and a dangling link's target is created
    for target in ["old.iso", "new.iso"] {
        let link = work.path().join(format!("link-{target}"));
        symlink(Path::new("store").join(target), &link).unwrap();
        pack(&src, &link);
        assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_unpacks_to(&store.join(target), &src);
    }

    let inner_link = src.join("link.iso");
    symlink("../store/old.iso", &inner_link).unwrap();
    pack(&src, &inner_link);
    let ls = xdvdfs(&["ls".as_ref(), store.join("old.iso").as_ref()]);
    assert_success(&ls);
    let listing = String::from_utf8(ls.stdout).unwrap();
    assert!(!listing.contains("partial"), "ls printed:\n{listing}");

    let left: Vec<_> = fs::read_dir(&store)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().ends_with(".partial"))
        .collect();
    assert!(left.is_empty(), "{left:?} were left behind");
}

#[test]
fn exiso_r_restores_input_when_replace_fails() {
    let work = tempdir().unwrap();
    let src = work.path().join("src");
    let img = work.path().join("g.iso");
    let out = work.path().join("x").join("out");
    write_tree(&src, &[("f", b"data")], &[]);
    pack(&src, &img);
    let packed = fs::read(&img).unwrap();
    // A directory where the output goes makes the final rename fail. `-d`
    // places it beside `out` (exiso_compat.rs), so block both spots.
    for blocker in [out.join("g.iso"), out.with_file_name("g.iso")] {
        fs::create_dir_all(blocker.join("keep")).unwrap();
    }

    assert_error(&xdvdfs(&[
        "extract-xiso".as_ref(),
        "-d".as_ref(),
        out.as_ref(),
        "-r".as_ref(),
        img.as_ref(),
    ]));
    assert!(fs::read(&img).unwrap() == packed, "g.iso changed");
    assert!(!img.with_added_extension("old").exists());
    for partial in [
        out.join("g.iso.partial"),
        out.with_file_name("g.iso.partial"),
    ] {
        assert!(!partial.exists(), "{} was left behind", partial.display());
    }
}

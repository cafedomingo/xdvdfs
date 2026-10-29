use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::NamedTempFile;

pub fn xdvdfs(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xdvdfs"))
        .args(args)
        .output()
        .expect("failed to run xdvdfs")
}

pub fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "xdvdfs exited with {}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

pub fn write_tree(root: &Path, files: &[(&str, &[u8])], empty_dirs: &[&str]) {
    fs::create_dir_all(root).unwrap();
    for dir in empty_dirs {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    for (name, data) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    }
}

/// Maps every path under `root` to its contents, or `None` for a directory.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, tree: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                tree.insert(relative, None);
                walk(root, &path, tree);
            } else {
                tree.insert(relative, Some(fs::read(&path).unwrap()));
            }
        }
    }

    let mut tree = BTreeMap::new();
    walk(root, root, &mut tree);
    tree
}

pub fn assert_trees_equal(a: &Path, b: &Path) {
    let (left, right) = (snapshot(a), snapshot(b));
    for path in left.keys().chain(right.keys()) {
        assert!(
            left.get(path) == right.get(path),
            "{} differs between {} and {}",
            path.display(),
            a.display(),
            b.display()
        );
    }
}

/// A temp file with `xiso` starting at byte `offset`, as in a full-disc dump.
pub fn place_at_offset(xiso: &Path, offset: u64) -> NamedTempFile {
    let mut out = NamedTempFile::new().unwrap();
    out.as_file().set_len(offset).unwrap();
    out.seek(SeekFrom::Start(offset)).unwrap();
    io::copy(&mut File::open(xiso).unwrap(), &mut out).unwrap();
    out
}

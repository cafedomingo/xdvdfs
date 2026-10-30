use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::Context;
use xdvdfs::blockdev::BlockDeviceRead;
use xdvdfs::read::DATA_CHUNK_LEN;

use crate::cmd_pack::{check_image_path, output_paths, replace, sync};

/// A FATX file must be under 4 GiB. For CSO this is `ciso.py`'s threshold:
/// a part ends after the first block that takes it past this size.
pub const FATX_SPLIT_SIZE: u64 = 0xFFBF_6000;

/// Part `n` of the image at `dir/name.ext`: `dir/name.n.cso`, since only
/// `.cso` files open as CSO
fn part_path(base: &Path, n: usize) -> PathBuf {
    base.with_extension(format!("{n}.cso"))
}

/// The parts of an image split across files, each written to a `.partial`
/// file and renamed into place once all are written.
pub struct SplitOutput {
    source: PathBuf,
    base: PathBuf,
    /// Each part created so far, and its `.partial` file
    parts: Vec<(PathBuf, PathBuf)>,
}

impl SplitOutput {
    /// For an image from the absolute `source` at `base`
    pub fn new(source: &Path, base: &Path) -> Self {
        Self {
            source: source.to_owned(),
            base: base.to_owned(),
            parts: Vec::new(),
        }
    }

    pub fn create_part(&mut self, n: usize) -> anyhow::Result<BufWriter<File>> {
        let (image_path, partial_path) = output_paths(&self.source, &part_path(&self.base, n))?;
        let file = File::create(&partial_path)?;
        self.parts.push((image_path, partial_path));
        Ok(BufWriter::with_capacity(DATA_CHUNK_LEN, file))
    }

    /// Syncs `parts`, the writers from `create_part` in order, and renames
    /// them into place. Then removes any later parts left by an earlier,
    /// larger image, since attach.xbe mounts every image in a folder.
    pub fn finish(self, parts: Vec<BufWriter<File>>) -> anyhow::Result<()> {
        let result = self.replace_parts(parts);
        if result.is_err() {
            self.remove_partials();
        }
        result
    }

    fn replace_parts(&self, parts: Vec<BufWriter<File>>) -> anyhow::Result<()> {
        let stale = self.stale_parts()?;
        for part in parts {
            sync(part)?;
        }
        for (image_path, partial_path) in &self.parts {
            replace(partial_path, image_path)?;
        }
        for path in stale {
            fs::remove_file(&path)
                .with_context(|| format!("Failed to remove {}", path.display()))?;
        }

        Ok(())
    }

    /// The parts after the last one created, each checked against the source
    fn stale_parts(&self) -> anyhow::Result<Vec<PathBuf>> {
        let mut stale = Vec::new();
        for n in self.parts.len() + 1.. {
            let path = part_path(&self.base, n);
            if fs::symlink_metadata(&path).is_err() {
                break;
            }
            check_image_path(&self.source, &path)?;
            stale.push(path);
        }

        Ok(stale)
    }

    pub fn remove_partials(&self) {
        for (_, partial_path) in &self.parts {
            let _ = fs::remove_file(partial_path);
        }
    }
}

/// Reads the parts of a split image back to back
pub struct SplitFileReader {
    parts: Vec<File>,
    part_lens: Vec<u64>,
}

impl SplitFileReader {
    /// Opens `first_part`, `dir/name.1.cso`, and each of `dir/name.2.cso`,
    /// `dir/name.3.cso`, … that exists after it
    pub fn open(first_part: &Path) -> io::Result<Self> {
        let base = first_part.with_extension("");
        let mut parts = vec![File::open(first_part)?];
        for n in 2.. {
            match File::open(part_path(&base, n)) {
                Ok(part) => parts.push(part),
                Err(err) if err.kind() == io::ErrorKind::NotFound => break,
                Err(err) => return Err(err),
            }
        }

        let part_lens = parts
            .iter()
            .map(|part| Ok(part.metadata()?.len()))
            .collect::<io::Result<_>>()?;
        Ok(Self { parts, part_lens })
    }

    pub fn part_lens(&self) -> &[u64] {
        &self.part_lens
    }
}

impl BlockDeviceRead for SplitFileReader {
    fn read(&mut self, mut offset: u64, mut buffer: &mut [u8]) -> io::Result<()> {
        for (part, &len) in self.parts.iter_mut().zip(&self.part_lens) {
            if buffer.is_empty() {
                break;
            }
            if offset >= len {
                offset -= len;
                continue;
            }

            let read_len = (len - offset).min(buffer.len() as u64) as usize;
            let (head, rest) = buffer.split_at_mut(read_len);
            BlockDeviceRead::read(part, offset, head)?;
            buffer = rest;
            offset = 0;
        }

        if buffer.is_empty() {
            Ok(())
        } else {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }
}

#[cfg(test)]
mod test {
    use std::fs;
    use std::io::{ErrorKind, Write};
    use std::path::Path;

    use xdvdfs::blockdev::BlockDeviceRead;

    use super::{part_path, SplitFileReader, SplitOutput};

    #[test]
    fn test_split_output_two_parts() {
        let dir = tempfile::tempdir().unwrap();
        let dir = fs::canonicalize(dir.path()).unwrap();
        let source = dir.join("src");
        fs::create_dir(&source).unwrap();

        let mut output = SplitOutput::new(&source, &dir.join("out.iso"));
        let mut parts = Vec::new();
        for (n, data) in [(1, b"one"), (2, b"two")] {
            let mut part = output.create_part(n).unwrap();
            part.write_all(data).unwrap();
            parts.push(part);
        }
        output.finish(parts).unwrap();

        let mut names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["out.1.cso", "out.2.cso", "src"]);
        assert_eq!(fs::read(dir.join("out.1.cso")).unwrap(), b"one");
        assert_eq!(fs::read(dir.join("out.2.cso")).unwrap(), b"two");

        let reader = SplitFileReader::open(&dir.join("out.1.cso")).unwrap();
        assert_eq!(reader.part_lens(), [3, 3]);
    }

    #[test]
    fn test_part_path_ends_in_cso() {
        for (base, part) in [
            ("out", "out.2.cso"),
            ("out.iso", "out.2.cso"),
            ("My.Game.cso", "My.Game.2.cso"),
            ("My.Game.1", "My.Game.2.cso"),
        ] {
            assert_eq!(part_path(Path::new(base), 2), Path::new(part));
        }
    }

    #[test]
    fn test_split_reader_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let data: Vec<u8> = (0..8000u32).map(|i| (i % 251) as u8).collect();
        fs::write(dir.path().join("x.1.cso"), &data[..5000]).unwrap();
        fs::write(dir.path().join("x.2.cso"), &data[5000..]).unwrap();

        let mut reader = SplitFileReader::open(&dir.path().join("x.1.cso")).unwrap();
        assert_eq!(reader.part_lens(), [5000, 3000]);

        let mut all = vec![0; data.len()];
        reader.read(0, &mut all).unwrap();
        assert_eq!(all, data);

        let mut span = [0; 20];
        reader.read(4990, &mut span).unwrap();
        assert_eq!(span, data[4990..5010]);

        let past_end = reader.read(7990, &mut span);
        assert_eq!(past_end.unwrap_err().kind(), ErrorKind::UnexpectedEof);
    }
}

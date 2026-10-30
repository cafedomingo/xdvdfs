use std::collections::BTreeMap;

use super::{FileEntry, FileType, FilesystemCopier, FilesystemHierarchy, ImagePath};
use crate::blockdev::BlockDeviceWrite;
use crate::xbe::patch_media_check;

const XBE_EXTENSION: &str = "xbe";

/// A source whose `.xbe` files read with their media check patched if `patch`,
/// else the source itself
pub struct MediaPatchFilesystem<F> {
    inner: F,
    patch: bool,
    /// The length of each `.xbe` that `read_dir` listed, which a ranged copy doesn't give
    xbe_lens: BTreeMap<ImagePath, u64>,
    /// Compression reads a file in ranges, which can split a media check, so
    /// the last XBE read is kept whole
    cached: Option<(ImagePath, Vec<u8>)>,
}

impl<F> MediaPatchFilesystem<F> {
    pub fn new(inner: F, patch: bool) -> Self {
        Self {
            inner,
            patch,
            xbe_lens: BTreeMap::new(),
            cached: None,
        }
    }
}

fn is_xbe(entry: &FileEntry) -> bool {
    entry.file_type == FileType::File
        && entry
            .name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(XBE_EXTENSION))
}

impl<F: FilesystemHierarchy> FilesystemHierarchy for MediaPatchFilesystem<F> {
    fn read_dir(&mut self, dir: &ImagePath) -> crate::Result<Vec<FileEntry>> {
        let entries = self.inner.read_dir(dir)?;
        if self.patch {
            let xbes = entries.iter().filter(|entry| is_xbe(entry));
            self.xbe_lens
                .extend(xbes.map(|entry| (dir.join(&entry.name), entry.len)));
        }
        Ok(entries)
    }
}

impl<BDW, F> FilesystemCopier<BDW> for MediaPatchFilesystem<F>
where
    BDW: BlockDeviceWrite + ?Sized,
    F: FilesystemCopier<BDW> + FilesystemCopier<[u8]>,
{
    fn copy_file_in(
        &mut self,
        src: &ImagePath,
        dest: &mut BDW,
        input_offset: u64,
        output_offset: u64,
        size: u64,
    ) -> crate::Result<u64> {
        let Some(&len) = self.xbe_lens.get(src) else {
            return self
                .inner
                .copy_file_in(src, dest, input_offset, output_offset, size);
        };
        if size == 0 {
            return Ok(0);
        }

        let xbe = match &mut self.cached {
            Some((path, xbe)) if *path == *src => xbe,
            cached => {
                let mut xbe = vec![0; len as usize];
                self.inner
                    .copy_file_in(src, xbe.as_mut_slice(), 0, 0, len)?;
                patch_media_check(&mut xbe);
                &mut cached.insert((src.clone(), xbe)).1
            }
        };

        let end = (input_offset + size).min(len);
        let data = &xbe[input_offset as usize..end as usize];
        dest.write(output_offset, data)?;
        Ok(data.len() as u64)
    }
}

#[cfg(test)]
mod test {
    use crate::write::fs::{FilesystemCopier, FilesystemHierarchy, ImagePath, MemoryFilesystem};
    use crate::xbe::{MEDIA_CHECK, MEDIA_CHECK_PATCH};

    use super::MediaPatchFilesystem;

    #[test]
    fn test_media_patch_across_ranges() {
        let at = 6;
        let mut xbe = vec![0; 20];
        xbe[at..][..MEDIA_CHECK.len()].copy_from_slice(&MEDIA_CHECK);
        let mut memfs = MemoryFilesystem::default();
        memfs.create("/default.xbe", &xbe);
        let mut fs = MediaPatchFilesystem::new(memfs, true);
        fs.read_dir(&ImagePath::root()).unwrap();

        let path = ImagePath::from("/default.xbe");
        let split = at + 3;
        let mut out = vec![0; xbe.len()];
        let (head, tail) = out.split_at_mut(split);
        let head_len = head.len() as u64;
        let tail_len = tail.len() as u64;
        assert_eq!(
            fs.copy_file_in(&path, head, 0, 0, head_len).unwrap(),
            head_len
        );
        assert_eq!(
            fs.copy_file_in(&path, tail, head_len, 0, tail_len).unwrap(),
            tail_len
        );

        xbe[at + MEDIA_CHECK.len() - 1] = MEDIA_CHECK_PATCH;
        assert_eq!(out, xbe);
    }

    #[test]
    fn test_media_patch_empty_xbe() {
        let mut memfs = MemoryFilesystem::default();
        memfs.touch("/default.xbe");
        let mut fs = MediaPatchFilesystem::new(memfs, true);
        fs.read_dir(&ImagePath::root()).unwrap();

        let copied = fs.copy_file_in(&"/default.xbe".into(), &mut [], 0, 0, 0);
        assert_eq!(copied.unwrap(), 0);
    }
}

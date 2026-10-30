use core::ops::RangeBounds;
use std::io;

#[cfg(test)]
use crate::write::fs::PathRef;
use crate::{blockdev::BlockDeviceWrite, layout, write::fs::PathVec};

/// Contents for a region of the sector linear block device
/// The sector offset is specified as the key in the contents tree,
/// and the sector lengt
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectorLinearBlockRegion {
    // Slice of data filling the region
    // Its length should be a multiple of sector size
    RawData(Box<[u8]>),
    // A file of `len` bytes, zero-padded to the end of its last sector
    File { path: PathVec, len: u64 },
}

/// Contents of a single sector within the sector linear block device
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectorLinearBlockSectorContents<'a> {
    RawData(&'a [u8]),
    File(PathRef<'a>),
    Empty,
}

impl SectorLinearBlockRegion {
    pub fn size_sectors(&self) -> u64 {
        let used_sectors = match self {
            Self::RawData(data) => {
                let len = <[u8]>::len(data);
                assert_eq!(len % (layout::SECTOR_SIZE as usize), 0);
                len.div_ceil(layout::SECTOR_SIZE as usize) as u64
            }
            Self::File { len, .. } => len.div_ceil(layout::SECTOR_SIZE_U64),
        };

        // An entry must occupy at least the sector it exists at,
        // even if it fills it with no data
        core::cmp::max(used_sectors, 1)
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_sectors() * layout::SECTOR_SIZE as u64
    }

    #[cfg(test)]
    fn sector_contents(&self, sector_offset: u64) -> SectorLinearBlockSectorContents<'_> {
        assert!(sector_offset < self.size_sectors());
        match self {
            Self::RawData(data) => {
                let sector_size = layout::SECTOR_SIZE as usize;
                let start = (sector_offset as usize) * sector_size;
                let end = start + sector_size;
                SectorLinearBlockSectorContents::RawData(&data[start..end])
            }
            Self::File { path, .. } => SectorLinearBlockSectorContents::File(path.into()),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SectorLinearBlockDevice {
    pub(super) contents: std::collections::BTreeMap<u64, SectorLinearBlockRegion>,
}

impl SectorLinearBlockDevice {
    pub fn num_sectors(&self) -> u64 {
        self.contents
            .last_key_value()
            .map(|(sector, contents)| *sector + contents.size_sectors())
            .unwrap_or(0)
    }

    pub fn size(&self) -> u64 {
        self.num_sectors() * (layout::SECTOR_SIZE as u64)
    }

    fn get_or_empty(&self, sector: u64) -> Option<(u64, &SectorLinearBlockRegion)> {
        let index = sector;
        self.contents
            .range(..=index)
            .next_back()
            .filter(|(sector, data)| index >= **sector && index < **sector + data.size_sectors())
            .map(|(sector, data)| (*sector, data))
    }

    #[cfg(test)]
    pub(crate) fn get(&self, sector: u64) -> SectorLinearBlockSectorContents<'_> {
        let index = sector;
        self.get_or_empty(index)
            .inspect(|(sector, _)| assert!(index >= *sector))
            .map(|(sector, data)| data.sector_contents(index - sector))
            .unwrap_or(SectorLinearBlockSectorContents::Empty)
    }

    pub fn sector_range<R: RangeBounds<u64>>(
        &self,
        range: R,
    ) -> impl Iterator<Item = (u64, &SectorLinearBlockRegion)> {
        use core::ops::Bound;
        let start_incl_bound = match range.start_bound() {
            Bound::Included(bound) => Some(*bound),
            Bound::Excluded(bound) => Some(*bound + 1),
            Bound::Unbounded => None,
        };

        let range_iter = self
            .contents
            .range(range)
            .map(|(sector, data)| (*sector, data));

        // Include data overlapping the start bound, if applicable.
        // Exclude if the sector is the same as the first bound sector,
        // as it will be included in the range_iter already.
        let mut start_incl_data = start_incl_bound
            .and_then(|bound| self.get_or_empty(bound))
            .filter(|(sector, _)| Some(*sector) != start_incl_bound);
        core::iter::from_fn(move || start_incl_data.take()).chain(range_iter)
    }

    /// The sector starting at byte `offset`
    pub(super) fn sector_at(offset: u64) -> io::Result<u64> {
        if !offset.is_multiple_of(layout::SECTOR_SIZE_U64) {
            let what = format!("offset {offset} doesn't start a sector");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, what));
        }

        Ok(offset / layout::SECTOR_SIZE_U64)
    }

    pub(super) fn insert(
        &mut self,
        sector: u64,
        region: SectorLinearBlockRegion,
    ) -> io::Result<()> {
        // Regions never overlap, so only the last one starting before `end`
        // can reach past `sector`
        let end = sector + region.size_sectors();
        let overlaps = self
            .contents
            .range(..end)
            .next_back()
            .is_some_and(|(start, earlier)| start + earlier.size_sectors() > sector);
        if overlaps {
            let what = format!("sectors {sector}..{end} overlap an earlier write");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, what));
        }

        self.contents.insert(sector, region);
        Ok(())
    }
}

impl BlockDeviceWrite for SectorLinearBlockDevice {
    fn write(&mut self, offset: u64, buffer: &[u8]) -> io::Result<()> {
        let sector = Self::sector_at(offset)?;

        let mut data = buffer.to_vec();
        data.resize(
            buffer.len().next_multiple_of(layout::SECTOR_SIZE as usize),
            0,
        );
        self.insert(
            sector,
            SectorLinearBlockRegion::RawData(data.into_boxed_slice()),
        )
    }

    fn len(&mut self) -> io::Result<u64> {
        Ok(self.size())
    }
}

#[cfg(test)]
mod test {
    use core::ops::Bound;

    use crate::write::fs::{PathVec, SectorLinearBlockRegion};

    use super::{SectorLinearBlockDevice, SectorLinearBlockSectorContents};

    fn file_region(sectors: u64) -> SectorLinearBlockRegion {
        SectorLinearBlockRegion::File {
            path: PathVec::default(),
            len: sectors * crate::layout::SECTOR_SIZE_U64,
        }
    }

    #[test]
    fn test_sector_linear_dev_size_end_raw_data() {
        let mut slbd = SectorLinearBlockDevice::default();
        let data = vec![0; 4096].into_boxed_slice();
        slbd.contents
            .insert(5, SectorLinearBlockRegion::RawData(data));
        assert_eq!(slbd.size(), 7 * 2048);
    }

    #[test]
    fn test_sector_linear_dev_size_end_file() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(
            5,
            SectorLinearBlockRegion::File {
                path: PathVec::default(),
                len: 2 * 2048 + 1,
            },
        );
        assert_eq!(slbd.size(), 8 * 2048);
    }

    #[test]
    fn test_sector_linear_dev_num_sectors_file() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(
            5,
            SectorLinearBlockRegion::File {
                path: PathVec::default(),
                len: 2048,
            },
        );
        assert_eq!(slbd.num_sectors(), 6);
    }

    #[test]
    fn test_sector_linear_dev_num_sectors_data() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(
            5,
            SectorLinearBlockRegion::RawData(vec![0; 2048].into_boxed_slice()),
        );
        assert_eq!(slbd.num_sectors(), 6);
    }

    #[test]
    fn test_sector_linear_dev_num_sectors_zero_length_file() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(
            5,
            SectorLinearBlockRegion::File {
                path: PathVec::default(),
                len: 0,
            },
        );
        assert_eq!(slbd.num_sectors(), 6);
    }

    #[test]
    fn test_sector_linear_dev_index_empty() {
        let slbd = SectorLinearBlockDevice::default();
        assert_eq!(slbd.get(0), SectorLinearBlockSectorContents::Empty);
    }

    #[test]
    fn test_sector_linear_dev_write_aligned() {
        use crate::blockdev::BlockDeviceWrite;

        let mut slbd = SectorLinearBlockDevice::default();
        slbd.write(2048, &[1, 2, 3, 4, 5])
            .expect("write should succeed");

        assert_eq!(slbd.get(0), SectorLinearBlockSectorContents::Empty);
        let SectorLinearBlockSectorContents::RawData(data) = slbd.get(1) else {
            panic!("Sector 1 should contain raw data");
        };
        assert_eq!(data[0..5], [1, 2, 3, 4, 5]);
        assert_eq!(slbd.get(2), SectorLinearBlockSectorContents::Empty);
    }

    #[test]
    fn test_sector_linear_dev_write_multi_sector() {
        use crate::blockdev::BlockDeviceWrite;

        let mut slbd = SectorLinearBlockDevice::default();
        let data = vec![10; 2048 * 2];
        slbd.write(2048, &data).expect("write should succeed");

        assert_eq!(slbd.get(0), SectorLinearBlockSectorContents::Empty);
        let SectorLinearBlockSectorContents::RawData(data) = slbd.get(1) else {
            panic!("Sector 1 should contain raw data");
        };
        assert_eq!(data[0..5], [10, 10, 10, 10, 10]);

        let SectorLinearBlockSectorContents::RawData(data) = slbd.get(2) else {
            panic!("Sector 2 should contain raw data");
        };
        assert_eq!(data[0..5], [10, 10, 10, 10, 10]);
        assert_eq!(slbd.get(3), SectorLinearBlockSectorContents::Empty);
    }

    #[test]
    fn test_sector_linear_dev_write_non_sequential() {
        use crate::blockdev::BlockDeviceWrite;

        let mut slbd = SectorLinearBlockDevice::default();
        let data = vec![10; 2048];
        slbd.write(4096, &data).expect("write should succeed");
        slbd.write(2048, &data).expect("write should succeed");

        assert_eq!(slbd.get(0), SectorLinearBlockSectorContents::Empty);
        let SectorLinearBlockSectorContents::RawData(data) = slbd.get(1) else {
            panic!("Sector 1 should contain raw data");
        };
        assert_eq!(data[0..5], [10, 10, 10, 10, 10]);

        let SectorLinearBlockSectorContents::RawData(data) = slbd.get(2) else {
            panic!("Sector 2 should contain raw data");
        };
        assert_eq!(data[0..5], [10, 10, 10, 10, 10]);
        assert_eq!(slbd.get(3), SectorLinearBlockSectorContents::Empty);
    }

    #[test]
    fn test_sector_linear_dev_write_rejects_bad_offsets() {
        use crate::blockdev::BlockDeviceWrite;
        use std::io::ErrorKind;

        let mut slbd = SectorLinearBlockDevice::default();
        slbd.write(2048, &[1; 4096]).expect("write should succeed");

        // Unaligned, overlapping the end, and empty at a written sector
        for (offset, len) in [(2049, 1), (4096, 1), (2048, 0)] {
            let res = slbd.write(offset, &vec![2; len]);
            assert!(
                res.is_err_and(|err| err.kind() == ErrorKind::InvalidInput),
                "write of {len} bytes at {offset}"
            );
        }
        assert_eq!(slbd.num_sectors(), 3);
    }

    #[test]
    fn test_sector_linear_range_iterator_start_not_contained() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(4, file_region(1));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd.sector_range(5..=9).collect();
        assert_eq!(
            result,
            &[
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_start_contained_by_prev() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd.sector_range(5..=9).collect();
        assert_eq!(
            result,
            &[
                (4, &file_region(2)),
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_unbounded_below() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(2, file_region(2));
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd.sector_range(..9).collect();
        assert_eq!(
            result,
            &[
                (2, &file_region(2)),
                (4, &file_region(2)),
                (6, &file_region(2)),
                (8, &file_region(1)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_unbounded_above() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(2, file_region(2));
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd.sector_range(5..).collect();
        assert_eq!(
            result,
            &[
                (4, &file_region(2)),
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_equal() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(2, file_region(2));
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd.sector_range(4..).collect();
        assert_eq!(
            result,
            &[
                (4, &file_region(2)),
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_excluded_start() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd
            .sector_range((Bound::Excluded(5), Bound::Included(9)))
            .collect();
        assert_eq!(
            result,
            &[
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }

    #[test]
    fn test_sector_linear_range_iterator_excluded_start_included_sector() {
        let mut slbd = SectorLinearBlockDevice::default();
        slbd.contents.insert(4, file_region(2));
        slbd.contents.insert(6, file_region(2));
        slbd.contents.insert(8, file_region(1));
        slbd.contents.insert(9, file_region(2));

        let result: Vec<(u64, &SectorLinearBlockRegion)> = slbd
            .sector_range((Bound::Excluded(4), Bound::Included(9)))
            .collect();
        assert_eq!(
            result,
            &[
                (4, &file_region(2)),
                (6, &file_region(2)),
                (8, &file_region(1)),
                (9, &file_region(2)),
            ]
        );
    }
}

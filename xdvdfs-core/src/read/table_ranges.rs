use alloc::collections::BTreeMap;

use crate::layout::{DirectoryEntryTable, SECTOR_SIZE};

/// Sector ranges of directory tables read so far, each with a value.
///
/// Valid images never overlap tables. Crafted ones can share or overlap them
/// to make a walk re-read the same entries exponentially or quadratically often.
pub(crate) struct TableRanges<T>(BTreeMap<u64, (u64, T)>);

impl<T> Default for TableRanges<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<T> TableRanges<T> {
    /// Returns the value of a recorded range that overlaps `table`'s, or
    /// records `table`'s range with `value()` and returns `None`.
    pub(crate) fn overlapping_or_insert(
        &mut self,
        table: &DirectoryEntryTable,
        value: impl FnOnce() -> T,
    ) -> Option<&T> {
        let start = u64::from(table.region.sector);
        let end = start + u64::from(table.region.size.div_ceil(SECTOR_SIZE));

        // Recorded ranges never overlap each other, so only the last one
        // starting before `end` can reach past `start`
        let overlapping = self
            .0
            .range(..end)
            .next_back()
            .filter(|(_, (recorded_end, _))| *recorded_end > start)
            .map(|(&recorded_start, _)| recorded_start);

        match overlapping {
            Some(recorded_start) => Some(&self.0[&recorded_start].1),
            None => {
                self.0.insert(start, (end, value()));
                None
            }
        }
    }
}

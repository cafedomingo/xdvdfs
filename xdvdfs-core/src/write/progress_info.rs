use super::fs::ImagePath;

#[allow(unused_variables)]
pub trait ProgressVisitor {
    fn directory_discovered(&mut self, num_entries: usize) {}

    fn entry_counts(&mut self, file_count: usize, dir_count: usize) {}

    fn directory_added(&mut self, path: &ImagePath, sector: u64) {}

    fn file_added(&mut self, path: &ImagePath, sector: u64) {}

    fn finished_copying_image_data(&mut self) {}

    fn finished(&mut self) {}
}

pub struct NoOpProgressVisitor;

impl ProgressVisitor for NoOpProgressVisitor {}

impl<T: FnMut(ProgressInfo)> ProgressVisitor for T {
    fn directory_discovered(&mut self, num_entries: usize) {
        (self)(ProgressInfo::DiscoveredDirectory(num_entries));
    }

    fn entry_counts(&mut self, file_count: usize, dir_count: usize) {
        (self)(ProgressInfo::FileCount(file_count));
        (self)(ProgressInfo::DirCount(dir_count));
    }

    fn directory_added(&mut self, path: &ImagePath, sector: u64) {
        (self)(ProgressInfo::DirAdded(path.clone(), sector));
    }

    fn file_added(&mut self, path: &ImagePath, sector: u64) {
        (self)(ProgressInfo::FileAdded(path.clone(), sector));
    }

    fn finished_copying_image_data(&mut self) {
        (self)(ProgressInfo::FinishedCopyingImageData);
    }

    fn finished(&mut self) {
        (self)(ProgressInfo::FinishedPacking);
    }
}

#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProgressInfo {
    DiscoveredDirectory(usize),
    FileCount(usize),
    DirCount(usize),
    DirAdded(ImagePath, u64),
    FileAdded(ImagePath, u64),
    FinishedCopyingImageData,
    FinishedPacking,
}

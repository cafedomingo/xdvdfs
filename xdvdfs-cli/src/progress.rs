use std::{
    borrow::Cow,
    fmt,
    io::{self, Write},
    path::Path,
};

use xdvdfs::write::{fs::ImagePath, img::ProgressVisitor};

/// Writes progress to stdout. The error is dropped, because a closed stdout
/// mustn't fail the image or leave a partial unpack.
pub fn print_progress(args: fmt::Arguments) {
    let _ = io::stdout().write_fmt(args);
}

pub struct StdIoProgressReporter<'source> {
    source_prefix: Cow<'source, str>,
    file_count: usize,
    progress_count: usize,
}

impl<'source> StdIoProgressReporter<'source> {
    pub fn new(source_path: &'source Path, is_dir: bool) -> Self {
        let source_prefix = if is_dir {
            source_path.to_string_lossy()
        } else {
            Cow::Borrowed("")
        };

        Self {
            source_prefix,
            file_count: 0,
            progress_count: 0,
        }
    }
}

impl<'source> ProgressVisitor for StdIoProgressReporter<'source> {
    fn entry_counts(&mut self, file_count: usize, dir_count: usize) {
        self.file_count += file_count;
        self.file_count += dir_count;
    }

    fn directory_added(&mut self, path: &ImagePath, sector: u64) {
        self.progress_count += 1;
        print_progress(format_args!(
            "[{}/{}] Added dir: {}{path} at sector {sector}\n",
            self.progress_count, self.file_count, self.source_prefix,
        ));
    }

    fn file_added(&mut self, path: &ImagePath, sector: u64) {
        self.progress_count += 1;
        print_progress(format_args!(
            "[{}/{}] Added file: {}{path} at sector {sector}\n",
            self.progress_count, self.file_count, self.source_prefix,
        ));
    }
}

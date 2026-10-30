use core::fmt::Display;

/// The attribute byte of a directory entry
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct DirentAttributes(pub u8);

impl DirentAttributes {
    const READ_ONLY: u8 = 0x01;
    const HIDDEN: u8 = 0x02;
    const SYSTEM: u8 = 0x04;
    const DIRECTORY: u8 = 0x10;
    const ARCHIVE: u8 = 0x20;
    const NORMAL: u8 = 0x80;

    pub fn attrs(self) -> u8 {
        self.0
    }

    fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    fn with(self, bit: u8, set: bool) -> Self {
        Self(if set { self.0 | bit } else { self.0 & !bit })
    }

    pub fn read_only(self) -> bool {
        self.has(Self::READ_ONLY)
    }

    pub fn hidden(self) -> bool {
        self.has(Self::HIDDEN)
    }

    pub fn system(self) -> bool {
        self.has(Self::SYSTEM)
    }

    pub fn directory(self) -> bool {
        self.has(Self::DIRECTORY)
    }

    pub fn archive(self) -> bool {
        self.has(Self::ARCHIVE)
    }

    pub fn normal(self) -> bool {
        self.has(Self::NORMAL)
    }

    pub fn with_directory(self, directory: bool) -> Self {
        self.with(Self::DIRECTORY, directory)
    }

    pub fn with_archive(self, archive: bool) -> Self {
        self.with(Self::ARCHIVE, archive)
    }
}

fn print_leading_space(f: &mut core::fmt::Formatter<'_>, has_prev: &mut bool) -> core::fmt::Result {
    if *has_prev {
        f.write_str(" ")?;
    }

    *has_prev = true;
    Ok(())
}

impl Display for DirentAttributes {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut has_prev = false;

        if self.directory() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("Directory")?;
        }

        if self.read_only() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("Read-Only")?;
        }

        if self.hidden() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("Hidden")?;
        }

        if self.system() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("System")?;
        }

        if self.archive() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("Archive")?;
        }

        if self.normal() {
            print_leading_space(f, &mut has_prev)?;
            f.write_str("Normal")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::DirentAttributes;

    #[test]
    fn test_layout_attributes_display() {
        assert_eq!(DirentAttributes(1 << 0).to_string(), "Read-Only");
        assert_eq!(DirentAttributes(1 << 1).to_string(), "Hidden");
        assert_eq!(DirentAttributes(1 << 2).to_string(), "System");
        assert_eq!(DirentAttributes(1 << 4).to_string(), "Directory");
        assert_eq!(DirentAttributes(1 << 5).to_string(), "Archive");
        assert_eq!(DirentAttributes(1 << 7).to_string(), "Normal");
        assert_eq!(
            DirentAttributes(0xff).to_string(),
            "Directory Read-Only Hidden System Archive Normal",
        );
    }
}

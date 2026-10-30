use encoding_rs::{EncoderResult, WINDOWS_1252};

pub fn cmp_ignore_case_utf8(a: &str, b: &str) -> core::cmp::Ordering {
    use core::cmp::Ordering;

    let mut a_chars = a.chars().map(|c| c.to_ascii_uppercase());
    let mut b_chars = b.chars().map(|c| c.to_ascii_uppercase());

    loop {
        let a_next = a_chars.next();
        let b_next = b_chars.next();
        if a_next.is_none() && b_next.is_none() {
            break Ordering::Equal;
        }

        let a = match a_next {
            Some(a) => a,
            None => break Ordering::Less,
        };

        let b = match b_next {
            Some(b) => b,
            None => break Ordering::Greater,
        };

        match a.cmp(&b) {
            Ordering::Equal => continue,
            cmp => break cmp,
        }
    }
}

/// Whether `name` is safe to use as a single path component on any host.
pub fn is_safe_name(name: &str) -> bool {
    // `:` because on Windows `Path::join("C:x")` discards the base path
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\\', ':', '\0'])
}

/// A name's length is stored in a byte
const MAX_NAME_LEN: usize = u8::MAX as usize;

/// Why a name can't be stored in a directory entry
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum NameError {
    /// Longer than 255 bytes in Windows-1252
    TooLong,
    /// Has a character that Windows-1252 lacks
    NotEncodable,
}

#[derive(Copy, Clone, Debug, Hash)]
pub struct DirentName<'alloc> {
    name: &'alloc str,
    encoded: [u8; MAX_NAME_LEN],
    encoded_len: u8,
}

impl<'alloc> DirentName<'alloc> {
    pub fn new(name: &'alloc str) -> Result<Self, NameError> {
        let mut encoded = [0; MAX_NAME_LEN];
        let (result, _, encoded_len) = WINDOWS_1252
            .new_encoder()
            .encode_from_utf8_without_replacement(name, &mut encoded, true);
        match result {
            EncoderResult::InputEmpty => {}
            EncoderResult::OutputFull => return Err(NameError::TooLong),
            EncoderResult::Unmappable(_) => return Err(NameError::NotEncodable),
        }

        Ok(Self {
            name,
            encoded,
            encoded_len: encoded_len as u8,
        })
    }

    pub fn get_name(&self) -> &str {
        self.name
    }

    pub fn get_encoded_name(&self) -> &[u8] {
        &self.encoded[..self.encoded_len as usize]
    }
}

impl PartialEq for DirentName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == core::cmp::Ordering::Equal
    }
}

impl Eq for DirentName<'_> {}

impl PartialOrd for DirentName<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DirentName<'_> {
    // Readers search tables with the same comparator
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        cmp_ignore_case_utf8(self.name, other.name)
    }
}

#[cfg(test)]
fn name_comparator_wrapper(a: &str, b: &str) -> core::cmp::Ordering {
    let a = DirentName::new(a).unwrap();
    let b = DirentName::new(b).unwrap();
    a.cmp(&b)
}

macro_rules! case_cmp_test {
    ($name:ident, $cmp:expr) => {
        #[cfg(test)]
        mod $name {
            #[test]
            fn test_str_ignore_case_alpha() {
                let mut strings = ["asdf", "GHJK", "bsdf", "AAAA"];
                strings.sort_by($cmp);

                assert_eq!(strings, ["AAAA", "asdf", "bsdf", "GHJK"]);
            }

            #[test]
            fn test_str_ignore_case_underscore_gt_alpha() {
                let mut strings = ["a_b", "abb"];
                strings.sort_by($cmp);

                assert_eq!(strings, ["abb", "a_b"]);
            }

            #[test]
            fn test_str_lt_gt_len() {
                let s1 = "abc";
                let s2 = "abcd";

                let mut lt = [s1, s2];
                lt.sort_by($cmp);
                assert_eq!(lt, [s1, s2]);

                let mut gt = [s2, s1];
                gt.sort_by($cmp);
                assert_eq!(gt, [s1, s2]);
            }

            #[test]
            fn test_str_ignore_case_ordering() {
                let mut strings = [
                    "FILE.ext",
                    "FILElower.xyz",
                    "FILElower-hyphen.xyz",
                    "FILElowerlonger.asd",
                    "FILE-hyphen.ext",
                    "abc",
                ];
                strings.sort_by($cmp);

                assert_eq!(
                    strings,
                    [
                        "abc",
                        "FILE-hyphen.ext",
                        "FILE.ext",
                        "FILElower-hyphen.xyz",
                        "FILElower.xyz",
                        "FILElowerlonger.asd",
                    ]
                );
            }

            #[test]
            fn test_str_ignore_case_beyond_ascii() {
                // `Œ` is 0x8c in Windows-1252, but U+0152 sorts after `é`
                let mut strings = ["Œuvre", "école", "zeta"];
                strings.sort_by($cmp);

                assert_eq!(strings, ["zeta", "école", "Œuvre"]);
            }
        }
    };
}

case_cmp_test!(test_layout_cmp_fn, |a, b| {
    crate::layout::cmp_ignore_case_utf8(a, b)
});
case_cmp_test!(test_layout_dirent_name_cmp, |a, b| {
    crate::layout::name::name_comparator_wrapper(a, b)
});

#[cfg(test)]
mod test {
    use super::{DirentName, NameError};

    #[test]
    fn test_layout_dirent_name_eq() {
        let n1 = DirentName::new("a").unwrap();
        let n2 = DirentName::new("A").unwrap();

        assert_eq!(n1, n2);
        assert_eq!(n1.partial_cmp(&n2), Some(core::cmp::Ordering::Equal));
    }

    #[test]
    fn test_layout_dirent_name_encode_max_len() {
        let name = "a".repeat(255);
        let name = DirentName::new(&name).unwrap();
        assert_eq!(name.get_encoded_name(), [b'a'; 255]);
    }

    #[test]
    fn test_layout_dirent_name_encode_over_max_len() {
        assert_eq!(DirentName::new(&"a".repeat(256)), Err(NameError::TooLong));
    }

    #[test]
    fn test_layout_dirent_name_max_len_is_encoded_len() {
        let name = "é".repeat(255);
        let name = DirentName::new(&name).unwrap();
        assert_eq!(name.get_encoded_name(), [0xe9; 255]);
        assert_eq!(DirentName::new(&"é".repeat(256)), Err(NameError::TooLong));
    }

    #[test]
    fn test_layout_dirent_name_long_names_differ_past_255_utf8_bytes() {
        let prefix = "é".repeat(128);
        let (a, b) = (format!("{prefix}a"), format!("{prefix}b"));
        assert!(DirentName::new(&a).unwrap() < DirentName::new(&b).unwrap());
    }

    #[test]
    fn test_layout_dirent_name_encode_invalid_char() {
        assert_eq!(DirentName::new("😀"), Err(NameError::NotEncodable));
    }
}

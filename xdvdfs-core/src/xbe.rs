/// A call to an XBE's media check, then `test eax, eax; jge`
pub const MEDIA_CHECK: [u8; 8] = [0xE8, 0xCA, 0xFD, 0xFF, 0xFF, 0x85, 0xC0, 0x7D];
/// `jmp`, in place of the `jge` ending `MEDIA_CHECK`
pub const MEDIA_CHECK_PATCH: u8 = 0xEB;

/// Rewrites the `jge` of each `MEDIA_CHECK` in `data` as a `jmp`, and returns
/// how many it patched.
pub fn patch_media_check(data: &mut [u8]) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(at) = data[start..]
        .windows(MEDIA_CHECK.len())
        .position(|window| window == MEDIA_CHECK)
    {
        start += at + MEDIA_CHECK.len();
        data[start - 1] = MEDIA_CHECK_PATCH;
        count += 1;
    }

    count
}

#[cfg(test)]
mod test {
    use super::{MEDIA_CHECK, MEDIA_CHECK_PATCH, patch_media_check};

    #[test]
    fn test_patch_media_check() {
        let mut data = [MEDIA_CHECK.as_slice(), &[0; 100], &MEDIA_CHECK].concat();
        let mut expected = data.clone();
        expected[MEDIA_CHECK.len() - 1] = MEDIA_CHECK_PATCH;
        expected[data.len() - 1] = MEDIA_CHECK_PATCH;

        assert_eq!(patch_media_check(&mut data), 2);
        assert_eq!(data, expected);

        assert_eq!(patch_media_check(&mut data), 0);
        assert_eq!(data, expected);
    }
}

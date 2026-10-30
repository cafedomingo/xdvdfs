use super::{SECTOR_SIZE_U64, SECTOR_SIZE_USZ};

/// Byte offset of ECMA-119's volume descriptor set. XDVDFS leaves the sectors
/// below its own volume descriptor at sector 32 unused.
pub const ISO9660_DESCRIPTORS_OFFSET: u64 = 16 * SECTOR_SIZE_U64;
/// The primary volume descriptor and the set terminator, one sector each
pub const ISO9660_DESCRIPTORS_LEN: usize = 2 * SECTOR_SIZE_USZ;

const PRIMARY_TYPE: u8 = 1;
const TERMINATOR_TYPE: u8 = 0xff;
const STANDARD_IDENTIFIER: [u8; 5] = *b"CD001";
const DESCRIPTOR_VERSION: u8 = 1;
const HEADER_LEN: usize = 7;

const VOLUME_SPACE_SIZE_OFFSET: usize = 80;
const VOLUME_SET_OFFSET: usize = 120;
/// Volume set size 1, volume sequence number 1 and the logical block size, 2048,
/// each as a little-endian value followed by the same value big-endian
const VOLUME_SET: [u8; 12] = [1, 0, 0, 1, 1, 0, 0, 1, 0, 8, 8, 0];
/// The volume set, publisher, data preparer and application identifiers and the
/// copyright, abstract and bibliographic file identifiers, all left blank
const BLANK_IDENTIFIERS_OFFSET: usize = 190;
const DATES_OFFSET: usize = 813;
/// ECMA-119's "not specified" date: sixteen zero digits and a zero GMT offset
const UNSPECIFIED_DATE: &[u8; 17] = b"0000000000000000\0";
/// Creation, modification, expiration and effective dates
const DATE_COUNT: usize = 4;
const FILE_STRUCTURE_VERSION_OFFSET: usize = DATES_OFFSET + DATE_COUNT * UNSPECIFIED_DATE.len();
const FILE_STRUCTURE_VERSION: u8 = 1;

fn header(kind: u8) -> [u8; HEADER_LEN] {
    let [a, b, c, d, e] = STANDARD_IDENTIFIER;
    [kind, a, b, c, d, e, DESCRIPTOR_VERSION]
}

fn put(buf: &mut [u8], offset: usize, bytes: &[u8]) {
    buf[offset..offset + bytes.len()].copy_from_slice(bytes);
}

/// The descriptors, filled as extract-xiso fills them so that burning software
/// detects the 2048-byte sector size. The fields a strict ISO 9660 reader needs
/// (system and volume identifiers, root directory record, path tables) stay zero.
pub fn iso9660_descriptors(volume_space_sectors: u32) -> [u8; ISO9660_DESCRIPTORS_LEN] {
    let mut buf = [0; ISO9660_DESCRIPTORS_LEN];
    let (primary, terminator) = buf.split_at_mut(SECTOR_SIZE_USZ);

    put(primary, 0, &header(PRIMARY_TYPE));
    let size_le = volume_space_sectors.to_le_bytes();
    put(primary, VOLUME_SPACE_SIZE_OFFSET, &size_le);
    put(
        primary,
        VOLUME_SPACE_SIZE_OFFSET + size_le.len(),
        &volume_space_sectors.to_be_bytes(),
    );
    put(primary, VOLUME_SET_OFFSET, &VOLUME_SET);
    primary[BLANK_IDENTIFIERS_OFFSET..DATES_OFFSET].fill(b' ');
    for date in 0..DATE_COUNT {
        put(
            primary,
            DATES_OFFSET + date * UNSPECIFIED_DATE.len(),
            UNSPECIFIED_DATE,
        );
    }
    primary[FILE_STRUCTURE_VERSION_OFFSET] = FILE_STRUCTURE_VERSION;

    put(terminator, 0, &header(TERMINATOR_TYPE));
    buf
}

#[cfg(test)]
mod test {
    use super::iso9660_descriptors;

    #[test]
    fn test_layout_iso9660_descriptors() {
        let mut expected = [0u8; 0x1000];
        expected[..7].copy_from_slice(b"\x01CD001\x01");
        expected[0x50..0x58].copy_from_slice(&[4, 3, 2, 1, 1, 2, 3, 4]);
        expected[0x78..0x84].copy_from_slice(&[1, 0, 0, 1, 1, 0, 0, 1, 0, 8, 8, 0]);
        expected[0xbe..0x32d].fill(b' ');
        expected[0x32d..0x371].copy_from_slice(&b"0000000000000000\0".repeat(4));
        expected[0x371] = 1;
        expected[0x800..0x807].copy_from_slice(b"\xffCD001\x01");

        assert!(iso9660_descriptors(0x0102_0304) == expected);
    }
}

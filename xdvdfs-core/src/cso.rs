//! Xbox CSO images: a 24-byte header, an index of block offsets, then each
//! 2048-byte block stored raw or LZ4-compressed, laid out as MakeMHz's
//! `ciso.py` writes them for Project Stellar.

use std::fmt::Display;
use std::io::{self, Read, Seek, SeekFrom, Write};

use bincode::Options;
use serde::{Deserialize, Serialize};

use crate::blockdev::BlockDeviceRead;
use crate::layout::{SECTOR_SIZE, SECTOR_SIZE_U64, SECTOR_SIZE_USZ};
use crate::read::DATA_CHUNK_LEN;

const MAGIC: u32 = u32::from_le_bytes(*b"CISO");
const HEADER_LEN: u32 = 24;
const VERSION: u8 = 2;
const BLOCK_LEN: u32 = SECTOR_SIZE;
const BLOCK_LEN_U64: u64 = SECTOR_SIZE_U64;
const BLOCK_LEN_USZ: usize = SECTOR_SIZE_USZ;
/// Blocks start on multiples of `1 << ALIGNMENT`, and index entries hold
/// their offsets shifted right by it
const ALIGNMENT: u8 = 2;
/// The largest alignment `CsoReader` accepts, so that shifting an entry
/// can't overflow
const MAX_ALIGNMENT: u8 = 31;
/// Set in the index entry of an LZ4-compressed block
const COMPRESSED: u32 = 1 << 31;
/// A compressed block starts with its length
const LEN_PREFIX: usize = size_of::<u32>();
/// `ciso.py` stores a block raw unless compressing it saves more than this,
/// counting the length prefix
const MIN_SAVING: usize = 12;
/// `ciso.py` pads every part to a multiple of this ("ATA block size * 2"),
/// adding a whole one to a part that is already aligned
const PART_PAD: u64 = 0x400;
const ZEROS: [u8; PART_PAD as usize] = [0; PART_PAD as usize];

#[derive(Deserialize, Serialize)]
struct Header {
    magic: u32,
    header_len: u32,
    image_len: u64,
    block_len: u32,
    version: u8,
    alignment: u8,
    reserved: [u8; 2],
}

fn bincode() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
}

fn invalid_data(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn invalid_block(block: usize, what: impl Display) -> io::Error {
    invalid_data(format!("CSO block {block}: {what}"))
}

fn invalid_input(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

fn block_offset(entry: u32, alignment: u8) -> u64 {
    u64::from(entry & !COMPRESSED) << alignment
}

/// Where one part's blocks are in a `CsoReader`'s input
struct Part {
    first_block: usize,
    start: u64,
    len: u64,
}

/// Reads the image held in a CSO
pub struct CsoReader<R> {
    inner: R,
    image_len: u64,
    alignment: u8,
    index: Vec<u32>,
    parts: Vec<Part>,
    /// The block decoded into `block`
    cached: Option<usize>,
    block: Vec<u8>,
    packed: Vec<u8>,
}

impl<R: BlockDeviceRead> CsoReader<R> {
    /// Opens the CSO whose parts, of lengths `part_lens`, `inner` reads back
    /// to back
    pub fn open(mut inner: R, part_lens: &[u64]) -> io::Result<Self> {
        let first_len = part_lens[0];
        if first_len < u64::from(HEADER_LEN) {
            return Err(invalid_data("CSO header runs past the end of the file"));
        }

        let mut header = [0; HEADER_LEN as usize];
        inner.read(0, &mut header)?;
        let header: Header = bincode().deserialize(&header).map_err(invalid_data)?;
        if header.magic != MAGIC {
            return Err(invalid_data("not a CSO image"));
        }
        if header.header_len != HEADER_LEN
            || header.version != VERSION
            || header.block_len != BLOCK_LEN
            || header.alignment > MAX_ALIGNMENT
        {
            return Err(invalid_data(format!(
                "unsupported CSO header: size {}, version {}, block size {}, alignment {}",
                header.header_len, header.version, header.block_len, header.alignment
            )));
        }
        if !header.image_len.is_multiple_of(BLOCK_LEN_U64) {
            return Err(invalid_data(format!(
                "CSO image size {} isn't a multiple of {BLOCK_LEN}",
                header.image_len
            )));
        }

        let entries = header.image_len / BLOCK_LEN_U64 + 1;
        let index_len = entries * size_of::<u32>() as u64;
        if u64::from(HEADER_LEN) + index_len > first_len {
            return Err(invalid_data(format!(
                "CSO index of {entries} entries runs past the end of the file"
            )));
        }
        let mut index = vec![0; index_len as usize];
        inner.read(HEADER_LEN.into(), &mut index)?;
        let index: Vec<u32> = index
            .as_chunks()
            .0
            .iter()
            .map(|entry| u32::from_le_bytes(*entry))
            .collect();

        let mut parts = vec![Part {
            first_block: 0,
            start: 0,
            len: first_len,
        }];
        let mut previous = 0;
        for (block, &entry) in index.iter().enumerate() {
            let offset = block_offset(entry, header.alignment);
            if offset < previous {
                let Some(&len) = part_lens.get(parts.len()) else {
                    return Err(invalid_block(block, "offset goes back, with no part left"));
                };
                let last = &parts[parts.len() - 1];
                let start = last.start + last.len;
                parts.push(Part {
                    first_block: block,
                    start,
                    len,
                });
            }
            if offset > parts[parts.len() - 1].len {
                return Err(invalid_block(block, "starts past the end of its part"));
            }
            previous = offset;
        }
        if parts.len() != part_lens.len() {
            return Err(invalid_data(format!(
                "CSO index uses {} of its {} parts",
                parts.len(),
                part_lens.len()
            )));
        }

        Ok(Self {
            inner,
            image_len: header.image_len,
            alignment: header.alignment,
            index,
            parts,
            cached: None,
            block: vec![0; BLOCK_LEN_USZ],
            packed: vec![0; LEN_PREFIX + BLOCK_LEN_USZ],
        })
    }

    fn load(&mut self, block: usize) -> io::Result<()> {
        if self.cached == Some(block) {
            return Ok(());
        }
        self.cached = None;

        let entry = self.index[block];
        let part = &self.parts[self.parts.partition_point(|part| part.first_block <= block) - 1];
        let offset = block_offset(entry, self.alignment);
        let room = part.len - offset;
        let at = part.start + offset;
        if entry & COMPRESSED == 0 {
            if room < BLOCK_LEN_U64 {
                return Err(invalid_block(block, "runs past the end of its part"));
            }
            self.inner.read(at, &mut self.block)?;
        } else {
            // Read by the block's own length: ciso.py and ciso 0.3.0 floor
            // the final index entry, so the index can end up to 3 bytes short
            // of the last block
            let packed_len = room.min(self.packed.len() as u64) as usize;
            let packed = &mut self.packed[..packed_len];
            self.inner.read(at, packed)?;
            let Some((len, data)) = packed.split_first_chunk::<LEN_PREFIX>() else {
                return Err(invalid_block(block, "runs past the end of its part"));
            };
            let len = u32::from_le_bytes(*len);
            if len > BLOCK_LEN {
                return Err(invalid_block(
                    block,
                    format!("compressed length {len} is over {BLOCK_LEN}"),
                ));
            }
            let Some(data) = data.get(..len as usize) else {
                return Err(invalid_block(block, "runs past the end of its part"));
            };
            let decoded = lz4_flex::block::decompress_into(data, &mut self.block)
                .map_err(|err| invalid_block(block, err))?;
            if decoded != BLOCK_LEN_USZ {
                return Err(invalid_block(
                    block,
                    format!("decodes to {decoded} bytes, not {BLOCK_LEN}"),
                ));
            }
        }

        self.cached = Some(block);
        Ok(())
    }
}

impl<R: BlockDeviceRead> BlockDeviceRead for CsoReader<R> {
    fn read(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        if offset + buffer.len() as u64 > self.image_len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }

        let mut done = 0;
        while done < buffer.len() {
            let position = offset + done as u64;
            let within = (position % BLOCK_LEN_U64) as usize;
            let len = (BLOCK_LEN_USZ - within).min(buffer.len() - done);
            self.load((position / BLOCK_LEN_U64) as usize)?;
            buffer[done..done + len].copy_from_slice(&self.block[within..within + len]);
            done += len;
        }

        Ok(())
    }
}

fn index_entry(position: u64) -> u32 {
    (position >> ALIGNMENT) as u32
}

fn align(out: &mut impl Write, position: u64) -> io::Result<u64> {
    let aligned = position.next_multiple_of(1 << ALIGNMENT);
    out.write_all(&ZEROS[..(aligned - position) as usize])?;
    Ok(aligned)
}

fn pad_part(out: &mut impl Write, position: u64) -> io::Result<()> {
    out.write_all(&ZEROS[..(PART_PAD - position % PART_PAD) as usize])
}

/// Writes the first `len` bytes of `input` as a CSO into `first`. Once
/// `first` passes `split_size`, the rest goes into a second part from
/// `second`, which is returned for the caller to finish.
///
/// `progress(done, total)` counts blocks.
pub fn write<W: Write + Seek>(
    input: &mut impl BlockDeviceRead,
    len: u64,
    split_size: u64,
    first: &mut W,
    second: impl FnOnce() -> io::Result<W>,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<Option<W>> {
    let blocks = len / BLOCK_LEN_U64;
    let header = Header {
        magic: MAGIC,
        header_len: HEADER_LEN,
        image_len: len,
        block_len: BLOCK_LEN,
        version: VERSION,
        alignment: ALIGNMENT,
        reserved: [0; 2],
    };
    first.write_all(&bincode().serialize(&header).map_err(io::Error::other)?)?;
    // Filled in once the block offsets are known
    let index_len = (blocks + 1) * size_of::<u32>() as u64;
    io::copy(&mut io::repeat(0).take(index_len), first)?;

    let mut index = Vec::new();
    let mut position = u64::from(HEADER_LEN) + index_len;
    let mut second = Some(second);
    let mut part_2 = None;
    let mut chunk = vec![0; DATA_CHUNK_LEN];
    let mut compressed = vec![0; lz4_flex::block::get_maximum_output_size(BLOCK_LEN_USZ)];
    for chunk_start in (0..len).step_by(DATA_CHUNK_LEN) {
        let chunk = &mut chunk[..(len - chunk_start).min(DATA_CHUNK_LEN as u64) as usize];
        input.read(chunk_start, chunk)?;

        for block in chunk.as_chunks::<BLOCK_LEN_USZ>().0 {
            // ciso.py checks before aligning, so the block that crosses
            // `split_size` stays in part 1
            if position > split_size {
                let Some(second) = second.take() else {
                    return Err(invalid_input("image too large for two CSO parts"));
                };
                pad_part(first, position)?;
                part_2 = Some(second()?);
                position = 0;
            }

            let out = part_2.as_mut().unwrap_or(&mut *first);
            position = align(out, position)?;
            let mut entry = index_entry(position);
            let compressed_len =
                lz4_flex::block::compress_into(block, &mut compressed).map_err(io::Error::other)?;
            if LEN_PREFIX + compressed_len + MIN_SAVING < BLOCK_LEN_USZ {
                entry |= COMPRESSED;
                out.write_all(&(compressed_len as u32).to_le_bytes())?;
                out.write_all(&compressed[..compressed_len])?;
                position += (LEN_PREFIX + compressed_len) as u64;
            } else {
                out.write_all(block)?;
                position += BLOCK_LEN_U64;
            }

            index.push(entry);
            progress(index.len() as u64, blocks);
        }
    }

    // ciso.py floors the final entry; aligning first makes it exact
    let out = part_2.as_mut().unwrap_or(&mut *first);
    position = align(out, position)?;
    index.push(index_entry(position));
    pad_part(out, position)?;

    first.seek(SeekFrom::Start(HEADER_LEN.into()))?;
    for entry in index {
        first.write_all(&entry.to_le_bytes())?;
    }

    Ok(part_2)
}

#[cfg(test)]
mod test {
    use std::io::{self, Cursor, ErrorKind};

    use rand::{rngs::StdRng, RngCore, SeedableRng};

    use super::{
        write, CsoReader, ALIGNMENT, BLOCK_LEN, BLOCK_LEN_USZ, COMPRESSED, HEADER_LEN, LEN_PREFIX,
        MAGIC, PART_PAD, VERSION,
    };
    use crate::blockdev::BlockDeviceRead;

    /// `count` each of incompressible, compressible and all-zero blocks. It
    /// ends with a zero block, whose compressed form ends unaligned.
    fn mixed_image(count: usize) -> Vec<u8> {
        let mut rng = StdRng::seed_from_u64(7);
        let mut image = Vec::new();
        for _ in 0..count {
            let mut random = [0; BLOCK_LEN_USZ];
            rng.fill_bytes(&mut random);
            image.extend(random);
            image.extend((0..BLOCK_LEN_USZ).map(|i| (i % 7) as u8));
            image.extend([0; BLOCK_LEN_USZ]);
        }
        image
    }

    fn write_parts(image: &[u8], split_size: u64) -> io::Result<(Vec<u8>, Option<Vec<u8>>)> {
        let mut first = Cursor::new(Vec::new());
        let second = write(
            &mut Cursor::new(image),
            image.len() as u64,
            split_size,
            &mut first,
            || Ok(Cursor::new(Vec::new())),
            |_, _| {},
        )?;
        Ok((first.into_inner(), second.map(Cursor::into_inner)))
    }

    fn write_one(image: &[u8]) -> Vec<u8> {
        let (cso, second) = write_parts(image, u64::MAX).unwrap();
        assert!(second.is_none());
        cso
    }

    fn read_all(cso: &[u8], part_lens: &[u64]) -> io::Result<Vec<u8>> {
        let mut reader = CsoReader::open(Cursor::new(cso), part_lens)?;
        let mut image = vec![0; image_len(cso) as usize];
        reader.read(0, &mut image)?;
        Ok(image)
    }

    fn image_len(cso: &[u8]) -> u64 {
        u64::from_le_bytes(cso[8..16].try_into().unwrap())
    }

    fn entries(cso: &[u8]) -> Vec<u32> {
        let count = (image_len(cso) / u64::from(BLOCK_LEN)) as usize + 1;
        cso[HEADER_LEN as usize..][..4 * count]
            .as_chunks()
            .0
            .iter()
            .map(|entry| u32::from_le_bytes(*entry))
            .collect()
    }

    fn offset(entry: u32) -> u64 {
        u64::from(entry & !COMPRESSED) << ALIGNMENT
    }

    /// Where the block at `entry` of `part` ends
    fn block_end(part: &[u8], entry: u32) -> u64 {
        let at = offset(entry);
        if entry & COMPRESSED == 0 {
            return at + u64::from(BLOCK_LEN);
        }

        let len = u32::from_le_bytes(part[at as usize..][..LEN_PREFIX].try_into().unwrap());
        at + (LEN_PREFIX as u64) + u64::from(len)
    }

    fn set(cso: &[u8], at: usize, bytes: &[u8]) -> Vec<u8> {
        let mut cso = cso.to_vec();
        cso[at..at + bytes.len()].copy_from_slice(bytes);
        cso
    }

    fn set_entry(cso: &[u8], i: usize, entry: u32) -> Vec<u8> {
        set(cso, HEADER_LEN as usize + 4 * i, &entry.to_le_bytes())
    }

    fn stored(fill: u8) -> (bool, Vec<u8>) {
        (false, vec![fill; BLOCK_LEN_USZ])
    }

    /// A compressed block of `payload` with a length field of `len`
    fn compressed_as(len: u32, payload: &[u8]) -> (bool, Vec<u8>) {
        (true, [&len.to_le_bytes(), payload].concat())
    }

    fn compressed(payload: &[u8]) -> (bool, Vec<u8>) {
        compressed_as(payload.len() as u32, payload)
    }

    fn lz4(data: &[u8]) -> Vec<u8> {
        lz4_flex::block::compress(data)
    }

    /// A one-part CSO of `blocks`, each whether it is compressed and its bytes
    fn craft(blocks: &[(bool, Vec<u8>)]) -> Vec<u8> {
        let data_start = HEADER_LEN as usize + 4 * (blocks.len() + 1);
        let mut cso = vec![0; data_start];
        let mut index = Vec::new();
        for (is_compressed, bytes) in blocks {
            cso.resize(cso.len().next_multiple_of(1 << ALIGNMENT), 0);
            let flag = if *is_compressed { COMPRESSED } else { 0 };
            index.push((cso.len() as u32 >> ALIGNMENT) | flag);
            cso.extend(bytes);
        }
        cso.resize(cso.len().next_multiple_of(1 << ALIGNMENT), 0);
        index.push(cso.len() as u32 >> ALIGNMENT);

        let header = [
            &MAGIC.to_le_bytes()[..],
            &HEADER_LEN.to_le_bytes(),
            &(blocks.len() as u64 * u64::from(BLOCK_LEN)).to_le_bytes(),
            &BLOCK_LEN.to_le_bytes(),
            &[VERSION, ALIGNMENT, 0, 0],
        ]
        .concat();
        cso[..header.len()].copy_from_slice(&header);
        for (i, entry) in index.into_iter().enumerate() {
            cso = set_entry(&cso, i, entry);
        }
        cso
    }

    fn assert_invalid(cso: &[u8], part_lens: &[u64], case: &str) {
        let err = read_all(cso, part_lens).expect_err(case);
        assert_eq!(err.kind(), ErrorKind::InvalidData, "{case}: {err}");
    }

    fn assert_invalid_one(cso: &[u8], case: &str) {
        assert_invalid(cso, &[cso.len() as u64], case);
    }

    #[test]
    fn test_cso_roundtrip() {
        let image = mixed_image(4);
        let cso = write_one(&image);

        let index = entries(&cso);
        let (end, blocks) = index.split_last().unwrap();
        assert!(blocks.iter().any(|entry| entry & COMPRESSED != 0));
        assert!(blocks.iter().any(|entry| entry & COMPRESSED == 0));
        let data_end = block_end(&cso, blocks[blocks.len() - 1]);
        assert_eq!(offset(*end), data_end.next_multiple_of(1 << ALIGNMENT));
        assert!((1..=PART_PAD).contains(&(cso.len() as u64 - offset(*end))));
        assert_eq!(cso.len() as u64 % PART_PAD, 0);
        assert!(read_all(&cso, &[cso.len() as u64]).unwrap() == image);

        let mut reader = CsoReader::open(Cursor::new(&cso), &[cso.len() as u64]).unwrap();
        let mut span = vec![0; 3 * BLOCK_LEN_USZ];
        reader.read(1000, &mut span).unwrap();
        assert!(span == image[1000..][..span.len()]);

        let past_end = reader.read(image.len() as u64 - 10, &mut [0; 20]);
        assert_eq!(past_end.unwrap_err().kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn test_cso_rejects_crafted_input() {
        let block_1 = vec![1; BLOCK_LEN_USZ];
        let block_2 = vec![2; BLOCK_LEN_USZ];
        let valid = craft(&[stored(1), compressed(&lz4(&block_2))]);
        assert!(read_all(&valid, &[valid.len() as u64]).unwrap() == [block_1, block_2].concat());

        let header_cases: &[(&str, usize, &[u8])] = &[
            ("magic", 0, b"PISO"),
            ("header size 0", 4, &0u32.to_le_bytes()),
            ("block size 0", 16, &0u32.to_le_bytes()),
            ("version 1", 20, &[1]),
            ("alignment 32", 21, &[32]),
            ("alignment 64", 21, &[64]),
            ("size not a block multiple", 8, &(4097u64).to_le_bytes()),
            (
                "index past the file",
                8,
                &(1000 * u64::from(BLOCK_LEN)).to_le_bytes(),
            ),
            (
                "size near u64::MAX",
                8,
                &(!(u64::from(BLOCK_LEN) - 1)).to_le_bytes(),
            ),
        ];
        for (case, at, bytes) in header_cases {
            assert_invalid_one(&set(&valid, *at, bytes), case);
        }
        assert_invalid_one(&valid[..HEADER_LEN as usize - 1], "short header");

        let three = craft(&[stored(1), stored(2), stored(3)]);
        let index = entries(&three);
        let swapped = set_entry(&set_entry(&three, 1, index[2]), 2, index[1]);
        assert_invalid_one(&swapped, "decrease with one part");

        let padded = [&valid[..], &[0; PART_PAD as usize]].concat();
        assert_invalid(
            &padded,
            &[valid.len() as u64, PART_PAD],
            "two parts, no decrease",
        );

        let bad_blocks = [
            ("length over 2048", compressed(&vec![0; BLOCK_LEN_USZ + 1])),
            (
                "length with bit 31",
                compressed_as(COMPRESSED | BLOCK_LEN, &[5; BLOCK_LEN_USZ]),
            ),
            ("decodes short", compressed(&lz4(&[2; 100]))),
            ("not lz4", compressed(&[0xff; 8])),
        ];
        for (case, block) in bad_blocks {
            assert_invalid_one(&craft(&[stored(1), block]), case);
        }

        // Each file ends one byte short of its block, with the final entry
        // moved back inside it
        for (case, block) in [
            ("stored", stored(1)),
            ("compressed", compressed(&lz4(&[2; BLOCK_LEN_USZ]))),
        ] {
            let cso = craft(&[block]);
            let start = entries(&cso)[0];
            let cut = &cso[..block_end(&cso, start) as usize - 1];
            assert_invalid_one(&set_entry(cut, 1, start), case);
        }
        let cso = craft(&[stored(1)]);
        let past_end = (2 * cso.len() as u32) >> ALIGNMENT;
        let moved = set_entry(&set_entry(&cso, 0, past_end), 1, past_end);
        assert_invalid_one(&moved, "block past the end");

        // Cut inside part 1's last block, so it would read on into part 2
        let (part_1, part_2) = write_parts(&mixed_image(48), 64 * 1024).unwrap();
        let part_2 = part_2.unwrap();
        let index = entries(&part_1);
        let switch = (1..index.len())
            .find(|&i| offset(index[i]) < offset(index[i - 1]))
            .unwrap();
        let cut = offset(index[switch - 1]) as usize + 100;
        let joined = [&part_1[..cut], &part_2[..]].concat();
        assert_invalid(
            &joined,
            &[cut as u64, part_2.len() as u64],
            "block past its part",
        );
    }

    #[test]
    fn test_cso_split_layout() {
        const SPLIT: u64 = 64 * 1024;
        let image = mixed_image(48);
        assert!(image.len() as u64 > SPLIT);

        let mut calls = 0;
        let mut first = Cursor::new(Vec::new());
        let second = write(
            &mut Cursor::new(&image),
            image.len() as u64,
            SPLIT,
            &mut first,
            || {
                calls += 1;
                Ok(Cursor::new(Vec::new()))
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(calls, 1);
        let (part_1, part_2) = (first.into_inner(), second.unwrap().into_inner());

        let index = entries(&part_1);
        let switch = (1..index.len())
            .find(|&i| offset(index[i]) < offset(index[i - 1]))
            .unwrap();
        assert_eq!(offset(index[switch]), 0);
        assert!(offset(index[0]) > 0, "block 0 is in part 1");
        let part_1_end = block_end(&part_1, index[switch - 1]);
        assert!(part_1_end > SPLIT);
        assert!(block_end(&part_1, index[switch - 2]) <= SPLIT);

        let last = index.len() - 1;
        let part_2_end = offset(index[last]);
        assert_eq!(
            part_2_end,
            block_end(&part_2, index[last - 1]).next_multiple_of(1 << ALIGNMENT)
        );

        for (part, end) in [(&part_1, part_1_end), (&part_2, part_2_end)] {
            assert_eq!(part.len() as u64 % PART_PAD, 0);
            assert!((1..=PART_PAD).contains(&(part.len() as u64 - end)));
        }

        let joined = [&part_1[..], &part_2[..]].concat();
        let part_lens = [part_1.len() as u64, part_2.len() as u64];
        assert!(read_all(&joined, &part_lens).unwrap() == image);

        let mut rng = StdRng::seed_from_u64(7);
        let mut too_big = vec![0; 3 * SPLIT as usize];
        rng.fill_bytes(&mut too_big);
        let err = write_parts(&too_big, SPLIT).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }
}

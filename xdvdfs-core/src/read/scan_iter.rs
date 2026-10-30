use crate::blockdev::BlockDeviceRead;
use crate::layout::{
    DirectoryEntryNode, DirectoryEntryTable, SECTOR_SIZE, SECTOR_SIZE_U64, SECTOR_SIZE_USZ,
};
use crate::Error;
use std::io;

pub struct DirentScanIter<'a, BDR: BlockDeviceRead + ?Sized> {
    sector: usize,
    sector_buf: [u8; SECTOR_SIZE_USZ],
    offset: usize,
    end_sector: usize,
    dev: &'a mut BDR,
}

impl<BDR: BlockDeviceRead + ?Sized> DirentScanIter<'_, BDR> {
    fn next_sector(&mut self) -> io::Result<()> {
        self.offset = 0;
        self.sector += 1;

        if self.sector >= self.end_sector {
            // Don't bother reading sectors in that we don't care about
            return Ok(());
        }

        self.dev
            .read((self.sector as u64) * SECTOR_SIZE_U64, &mut self.sector_buf)
    }

    pub fn next_entry(&mut self) -> crate::Result<Option<DirectoryEntryNode>> {
        if self.sector >= self.end_sector {
            return Ok(None);
        }

        loop {
            // Invariant: offset must remain in bounds of sector_buf
            assert!(self.offset + 0xe < SECTOR_SIZE_USZ);

            let mut buf = [0; 0xe];
            let name_offset = self.offset + 0xe;
            buf.copy_from_slice(&self.sector_buf[self.offset..name_offset]);

            let img_offset = (self.sector as u64 * SECTOR_SIZE_U64) + self.offset as u64;
            let dirent = DirectoryEntryNode::deserialize(&buf, img_offset)
                .map_err(|_| Error::DeserializationFailed)?;
            let Some(mut dirent) = dirent else {
                // If we find an empty record, but we still have sectors to go,
                // advance the sector count and retry
                if self.sector + 1 < self.end_sector {
                    self.next_sector()?;
                    continue;
                }

                break Ok(None);
            };

            let name_len = dirent.node.dirent.filename_length as usize;
            let name = self
                .sector_buf
                .get(name_offset..(name_offset + name_len))
                .ok_or(Error::DeserializationFailed)?;
            dirent.name[0..name_len].copy_from_slice(name);

            // Dirent is valid, advance cursor before returning
            self.offset = name_offset + name_len;
            self.offset += (4 - (self.offset % 4)) % 4;

            if self.offset + 0xe >= SECTOR_SIZE_USZ {
                self.next_sector()?;
            }

            break Ok(Some(dirent));
        }
    }
}

impl DirectoryEntryTable {
    /// Scan the dirent tree iteratively, reading an entire sector at a time
    /// Order is not guaranteed, but reads are batched
    pub fn scan_dirent_tree<'a, BDR: BlockDeviceRead + ?Sized>(
        &self,
        dev: &'a mut BDR,
    ) -> crate::Result<DirentScanIter<'a, BDR>> {
        let Ok(sector_offset) = self.offset(0) else {
            // Return a dummy scan iterator that is 0xff filled.
            // This is considered to be an empty sector, and avoids
            // needing to read data or maintain other empty state.
            return Ok(DirentScanIter {
                sector: 0,
                sector_buf: [0xff; SECTOR_SIZE_USZ],
                offset: 0,
                end_sector: 0,
                dev,
            });
        };

        let mut sector_buf = [0; SECTOR_SIZE_USZ];
        dev.read(sector_offset, &mut sector_buf)?;

        let sector = self.region.sector as usize;
        Ok(DirentScanIter {
            sector,
            sector_buf,
            offset: 0,
            end_sector: sector + self.region.size.div_ceil(SECTOR_SIZE) as usize,
            dev,
        })
    }
}

#[cfg(test)]
mod test {
    use crate::{
        blockdev::BlockDeviceWrite,
        layout::{
            DirectoryEntryDiskData, DirectoryEntryDiskNode, DirectoryEntryNode,
            DirectoryEntryTable, DirentAttributes, DiskRegion,
        },
        read::DirentScanIter,
        write::{
            dirtab::{AvlDirectoryEntryTableBuilder, DirtabWriterBuffers},
            fs::{
                MemoryFilesystem, SectorLinearBlockDevice, SectorLinearBlockFilesystem,
                SectorLinearImage,
            },
            sector::SectorAllocator,
        },
        Error,
    };

    fn name_bytes_from(name: &str) -> [u8; 256] {
        assert!(name.len() <= 256);
        let mut out = [0u8; 256];
        out[..name.len()].copy_from_slice(name.as_bytes());
        out
    }

    #[test]
    fn test_read_scan_iter_empty_size() {
        let table = DirectoryEntryTable {
            region: DiskRegion { sector: 0, size: 0 },
        };
        let mut dev = [];

        let mut scan_iter = table
            .scan_dirent_tree(dev.as_mut_slice())
            .expect("Creating scan iter should not fail");
        assert!(matches!(
            scan_iter,
            DirentScanIter {
                sector: 0,
                offset: 0,
                end_sector: 0,
                ..
            }
        ));

        let next = scan_iter.next_entry();
        assert!(matches!(next, Ok(None)));
    }

    #[test]
    fn test_read_scan_iter_empty_filled() {
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };
        let mut dev = [0xffu8; 2048];

        let mut scan_iter = table
            .scan_dirent_tree(dev.as_mut_slice())
            .expect("Creating scan iter should not fail");
        assert!(matches!(
            scan_iter,
            DirentScanIter {
                sector: 0,
                offset: 0,
                end_sector: 1,
                ..
            }
        ));

        let next = scan_iter.next_entry();
        assert!(matches!(next, Ok(None)));
    }

    #[test]
    fn test_scan_dirent_tree_name_past_sector_end() {
        // A 0xe-byte header and 255-byte name, padded to a dword. Seven fit
        // in the sector; the eighth's header fits but its name runs past the end.
        const ENTRY_LEN: usize = 272;
        #[rustfmt::skip]
        let header = [
            0, 0, 0, 0,
            1, 0, 0, 0,
            2, 0, 0, 0,
            0, 255,
        ];
        let mut dev = [b'a'; 2048];
        for entry in dev.chunks_mut(ENTRY_LEN) {
            entry[..header.len()].copy_from_slice(&header);
        }
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 2048,
            },
        };

        let mut iter = table
            .scan_dirent_tree(dev.as_mut_slice())
            .expect("Creating scan iter should succeed");
        for _ in 0..7 {
            let next = iter
                .next_entry()
                .expect("Entries in the sector should read");
            assert!(next.is_some());
        }

        let next = iter.next_entry();
        assert!(matches!(next, Err(Error::DeserializationFailed)));
    }

    #[test]
    fn test_read_scan_iter_skip_null_entry() {
        let mut slbd = SectorLinearBlockDevice::default();
        let mut fs = SectorLinearBlockFilesystem::new(MemoryFilesystem::default());

        {
            #[rustfmt::skip]
            let dirtab = [
                0, 0, 0, 0,
                1, 0, 0, 0,
                2, 0, 0, 0,
                0xff, 1, b'A',
                0xff, // padding
                0, 0, 0, 0,
                1, 0, 0, 0,
                2, 0, 0, 0,
                0xff, 1, b'B',
                0xff, // padding
            ];
            slbd.write(2048, &dirtab).expect("write should succeed");
        }

        {
            #[rustfmt::skip]
            let dirtab = [
                0, 0, 0, 0,
                1, 0, 0, 0,
                2, 0, 0, 0,
                0xff, 1, b'C',
                0xff, // padding
            ];
            slbd.write(4096, &dirtab).expect("write should succeed");
            slbd.write(6144, &dirtab).expect("write should succeed");
        }

        let mut dev = SectorLinearImage::new(&slbd, &mut fs);
        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 1,
                size: 4096,
            },
        };

        let mut iter = table
            .scan_dirent_tree(&mut dev)
            .expect("Creating scan iter should succeed");

        let next = iter
            .next_entry()
            .expect("Error on next entry is unexpected");
        assert_eq!(
            next,
            Some(DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 0,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector: 1, size: 2 },
                        attributes: DirentAttributes(0xff),
                        filename_length: 1,
                    },
                },
                name: name_bytes_from("A"),
                offset: 2048,
            })
        );

        let next = iter
            .next_entry()
            .expect("Error on next entry is unexpected");
        assert_eq!(
            next,
            Some(DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 0,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector: 1, size: 2 },
                        attributes: DirentAttributes(0xff),
                        filename_length: 1,
                    },
                },
                name: name_bytes_from("B"),
                offset: 2048 + 0x10,
            })
        );

        let next = iter
            .next_entry()
            .expect("Error on next entry is unexpected");
        assert_eq!(
            next,
            Some(DirectoryEntryNode {
                node: DirectoryEntryDiskNode {
                    left_entry_offset: 0,
                    right_entry_offset: 0,
                    dirent: DirectoryEntryDiskData {
                        data: DiskRegion { sector: 1, size: 2 },
                        attributes: DirentAttributes(0xff),
                        filename_length: 1,
                    },
                },
                name: name_bytes_from("C"),
                offset: 4096,
            })
        );

        // Ensure reads don't extend into subsequent sectors
        let next = iter
            .next_entry()
            .expect("Error on next entry is unexpected");
        assert_eq!(next, None);
        let next = iter
            .next_entry()
            .expect("Error on next entry is unexpected");
        assert_eq!(next, None);
    }

    #[test]
    fn test_read_scan_iter_sector_rollover() {
        use std::collections::HashSet;

        let mut entry_names: Vec<String> = Vec::new();

        // Each entry is 0x10 (filename length of 2),
        // so 256 fills two sectors exactly.
        for i in 0..256 {
            entry_names.push(format!("{i:02x}"));
        }

        let mut dtb = AvlDirectoryEntryTableBuilder::default();
        for name in &entry_names {
            dtb.add_file(name, 10)
                .expect("Dirent insertion should succeed");
        }
        let mut dtb = dtb.build().expect("Building dirtab should succeed");
        let mut allocator = SectorAllocator::default();
        let mut buffers = DirtabWriterBuffers::default();
        dtb.disk_repr(&mut allocator, &mut buffers)
            .expect("Serializing dirtab should succeed");
        assert_eq!(buffers.dirtab_bytes.len(), 4096);

        let table = DirectoryEntryTable {
            region: DiskRegion {
                sector: 0,
                size: 4096,
            },
        };

        let mut iter = table
            .scan_dirent_tree(buffers.dirtab_bytes.as_mut_slice())
            .expect("Creating iter should succeed");

        let mut name_set = HashSet::new();
        for name in &entry_names {
            name_set.insert(name.as_str());
        }

        for _ in 0..256 {
            let next = iter
                .next_entry()
                .expect("Reading next entry should succeed")
                .expect("Next entry should exist");

            // The order of entries in the array is not guaranteed,
            // so use a set to ensure all names are pulled.
            let name_str = next.name_str().expect("Name should be deserializable");
            assert!(name_set.remove(name_str.as_ref()));
        }

        assert!(name_set.is_empty());

        let next = iter
            .next_entry()
            .expect("Reading next entry should succeed");
        assert_eq!(next, None);
        let next = iter
            .next_entry()
            .expect("Reading next entry should succeed");
        assert_eq!(next, None);
    }
}

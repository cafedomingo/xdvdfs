use sha3::{Digest, Sha3_256};
use std::collections::BTreeMap;

use crate::{
    blockdev::BlockDeviceRead,
    layout::{DirectoryEntryNode, VolumeDescriptor},
};

pub fn checksum<BDR: BlockDeviceRead + ?Sized>(
    dev: &mut BDR,
    volume: &VolumeDescriptor,
) -> crate::Result<[u8; 32]> {
    let mut hasher = Sha3_256::new();

    let tree = volume.root_table.file_tree(dev)?;
    let mut iter: BTreeMap<String, DirectoryEntryNode> = BTreeMap::new();

    for (dir, file) in tree {
        let path = format!("{}/{}", dir, file.name_str()?);
        iter.insert(path, file);
    }

    for (path, file) in iter {
        let is_dir = file.node.dirent.is_directory();

        hasher.update(path.as_bytes());

        if !is_dir {
            file.node
                .dirent
                .read_data_chunks(dev, |chunk| hasher.update(chunk))
                .map_err(|err| err.in_file(path.as_str()))?;
        }
    }

    let bytes = hasher.finalize();
    let output: [u8; 32] = bytes[..]
        .try_into()
        .expect("SHA256 output should be 32 bytes");
    Ok(output)
}

#[cfg(test)]
mod test {
    use sha3::{Digest, Sha3_256};

    use crate::{
        read::{read_volume, DATA_CHUNK_LEN},
        write::{
            fs::{
                MemoryFilesystem, SectorLinearBlockDevice, SectorLinearBlockFilesystem,
                SectorLinearImage,
            },
            img::{create_xdvdfs_image, NoOpProgressVisitor},
        },
    };

    use super::checksum;

    fn checksum_of(mut fs: MemoryFilesystem) -> [u8; 32] {
        let mut slbd = SectorLinearBlockDevice::default();
        let mut fs = SectorLinearBlockFilesystem::new(&mut fs);
        create_xdvdfs_image(&mut fs, &mut slbd, NoOpProgressVisitor)
            .expect("Image creation should succeed");

        let mut dev = SectorLinearImage::new(&slbd, &mut fs);

        let volume = read_volume(&mut dev).expect("Volume should exist");
        checksum(&mut dev, &volume).expect("Checksum should be computed successfully")
    }

    #[test]
    fn test_checksum() {
        let mut fs = MemoryFilesystem::default();
        fs.create("/a/b/c", "Hello World\n".as_bytes());
        fs.create("/a/b/d", "Test data\n".as_bytes());
        fs.create("/a/e", "xdvdfs checksum\n".as_bytes());
        fs.create("/f", "datadatadata\n".as_bytes());

        assert_eq!(
            checksum_of(fs),
            [
                154, 255, 59, 107, 3, 163, 32, 25, 37, 133, 40, 250, 132, 208, 149, 99, 59, 239,
                96, 26, 138, 31, 215, 39, 201, 103, 229, 48, 28, 220, 196, 78
            ]
        );
    }

    #[test]
    fn test_checksum_file_larger_than_chunk() {
        let big: Vec<u8> = (0..2 * DATA_CHUNK_LEN + 13)
            .map(|i| (i % 251) as u8)
            .collect();
        let mut fs = MemoryFilesystem::default();
        fs.create("/a/big", &big);
        fs.create("/small", b"small");

        let mut expected = Sha3_256::new();
        for part in [&b"/a"[..], b"/a/big", &big, b"/small", b"small"] {
            expected.update(part);
        }
        assert_eq!(checksum_of(fs), <[u8; 32]>::from(expected.finalize()));
    }
}

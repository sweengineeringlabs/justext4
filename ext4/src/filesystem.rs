//! Filesystem handle — open an image, read inodes by number.

use std::io::{Read, Seek, SeekFrom};

use spec::{GroupDescriptor, Inode, Superblock, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE};

use crate::error::Ext4Error;

/// Open ext4 image. Holds the reader plus the eagerly-loaded
/// superblock and group descriptor table. Subsequent inode reads
/// seek into the reader for the bytes; the GDT is cached in
/// memory because every inode lookup needs it.
#[derive(Debug)]
pub struct Filesystem<R> {
    reader: R,
    superblock: Superblock,
    gdt: Vec<GroupDescriptor>,
}

impl<R: Read + Seek> Filesystem<R> {
    /// Open an image. Reads the superblock at byte offset 1024,
    /// validates magic + sanity checks the layout fields, then
    /// loads the GDT immediately after the superblock block.
    pub fn open(mut reader: R) -> Result<Self, Ext4Error> {
        // ── superblock ─────────────────────────────────────────
        reader.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
        let mut sb_buf = vec![0u8; SUPERBLOCK_SIZE];
        reader.read_exact(&mut sb_buf)?;
        let superblock = Superblock::decode(&sb_buf)?;

        if superblock.blocks_per_group == 0 {
            return Err(Ext4Error::InvalidLayout {
                reason: "blocks_per_group is 0",
            });
        }
        if superblock.inodes_per_group == 0 {
            return Err(Ext4Error::InvalidLayout {
                reason: "inodes_per_group is 0",
            });
        }

        // ── group descriptor table ─────────────────────────────
        let block_size = superblock.block_size as u64;
        let gdt_block = (superblock.first_data_block as u64) + 1;
        let gdt_offset = gdt_block * block_size;
        let group_count = superblock.group_count();
        let entry_size = superblock.group_descriptor_size() as usize;
        let gdt_bytes = group_count as usize * entry_size;

        reader.seek(SeekFrom::Start(gdt_offset))?;
        let mut gdt_buf = vec![0u8; gdt_bytes];
        reader.read_exact(&mut gdt_buf)?;

        let mut gdt = Vec::with_capacity(group_count as usize);
        for i in 0..group_count as usize {
            let off = i * entry_size;
            gdt.push(GroupDescriptor::decode(
                &gdt_buf[off..off + entry_size],
                &superblock,
            )?);
        }

        Ok(Filesystem {
            reader,
            superblock,
            gdt,
        })
    }

    /// Borrow the decoded superblock.
    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }

    /// Borrow the group descriptor table.
    pub fn group_descriptor_table(&self) -> &[GroupDescriptor] {
        &self.gdt
    }

    /// Read and decode the inode with number `inode_number`.
    ///
    /// Inode numbering is 1-based — inode 0 is the kernel's "no
    /// inode" sentinel and is never valid. Inode 2 is conventionally
    /// the root directory.
    pub fn read_inode(&mut self, inode_number: u32) -> Result<Inode, Ext4Error> {
        if inode_number == 0 || inode_number > self.superblock.inodes_count {
            return Err(Ext4Error::InodeOutOfRange {
                inode: inode_number,
                max: self.superblock.inodes_count,
            });
        }

        let zero_based = inode_number - 1;
        let group = zero_based / self.superblock.inodes_per_group;
        let index_in_group = zero_based % self.superblock.inodes_per_group;

        let group_idx = group as usize;
        if group_idx >= self.gdt.len() {
            // Defensive — `inodes_count <= group_count *
            // inodes_per_group` should hold on a sane image, but
            // a corrupt superblock could violate it.
            return Err(Ext4Error::InvalidLayout {
                reason: "inode references non-existent group",
            });
        }

        let inode_table_block = self.gdt[group_idx].inode_table;
        let block_size = self.superblock.block_size as u64;
        let inode_size = self.superblock.inode_size as u64;
        let offset = inode_table_block * block_size + (index_in_group as u64) * inode_size;

        self.reader.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; inode_size as usize];
        self.reader.read_exact(&mut buf)?;
        Ok(Inode::decode(&buf, &self.superblock)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spec::{InodeFileType, EXT4_MAGIC};
    use std::io::Cursor;

    // ── test image construction ────────────────────────────────
    //
    // Builds a minimal valid ext4 image entirely in memory. One
    // group, 8 blocks total at 4 KiB / block (32 KiB image).
    // Layout:
    //   block 0: padding + superblock (at byte 1024)
    //   block 1: GDT (one 32-byte descriptor)
    //   block 2: block bitmap (zeroed)
    //   block 3: inode bitmap (zeroed)
    //   block 4-5: inode table (32 inodes * 256 bytes = 8 KiB)
    //   block 6-7: data blocks (unused in current tests)
    //
    // Field offsets are duplicated here (rather than re-exported
    // from spec::superblock) because they're internal to that
    // module. Test image construction is allowed to know the
    // wire format directly.

    const BLOCK_SIZE: usize = 4096;
    const NUM_BLOCKS: u32 = 8;
    const INODES_PER_GROUP: u32 = 32;
    const INODE_SIZE: u16 = 256;

    // Superblock field offsets (within the 1024-byte struct).
    const SB_INODES_COUNT: usize = 0x00;
    const SB_BLOCKS_COUNT_LO: usize = 0x04;
    const SB_FIRST_DATA_BLOCK: usize = 0x14;
    const SB_LOG_BLOCK_SIZE: usize = 0x18;
    const SB_BLOCKS_PER_GROUP: usize = 0x20;
    const SB_INODES_PER_GROUP: usize = 0x28;
    const SB_MAGIC: usize = 0x38;
    const SB_REV_LEVEL: usize = 0x4C;
    const SB_INODE_SIZE: usize = 0x58;

    // GDT entry field offsets (within a 32-byte entry).
    const GDT_BLOCK_BITMAP_LO: usize = 0x00;
    const GDT_INODE_BITMAP_LO: usize = 0x04;
    const GDT_INODE_TABLE_LO: usize = 0x08;

    // Inode field offsets (within the 256-byte rev-1 inode).
    const I_MODE: usize = 0x00;
    const I_LINKS_COUNT: usize = 0x1A;

    fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Construct a minimal valid ext4 image with the root inode
    /// (inode 2) populated to the caller's mode + links_count.
    /// Returns the raw bytes; wrap in a `Cursor` for IO.
    fn build_image(root_mode: u16, root_links: u16) -> Vec<u8> {
        let mut img = vec![0u8; BLOCK_SIZE * NUM_BLOCKS as usize];

        // Superblock at offset 1024 (within block 0).
        let sb_off = 1024;
        write_u32(&mut img, sb_off + SB_INODES_COUNT, INODES_PER_GROUP);
        write_u32(&mut img, sb_off + SB_BLOCKS_COUNT_LO, NUM_BLOCKS);
        write_u32(&mut img, sb_off + SB_FIRST_DATA_BLOCK, 0);
        write_u32(&mut img, sb_off + SB_LOG_BLOCK_SIZE, 2); // 4 KiB
        write_u32(&mut img, sb_off + SB_BLOCKS_PER_GROUP, NUM_BLOCKS);
        write_u32(&mut img, sb_off + SB_INODES_PER_GROUP, INODES_PER_GROUP);
        write_u16(&mut img, sb_off + SB_MAGIC, EXT4_MAGIC);
        write_u32(&mut img, sb_off + SB_REV_LEVEL, 1);
        write_u16(&mut img, sb_off + SB_INODE_SIZE, INODE_SIZE);

        // GDT at block 1 (32-byte entry, lo addresses only).
        let gdt_off = BLOCK_SIZE;
        write_u32(&mut img, gdt_off + GDT_BLOCK_BITMAP_LO, 2);
        write_u32(&mut img, gdt_off + GDT_INODE_BITMAP_LO, 3);
        write_u32(&mut img, gdt_off + GDT_INODE_TABLE_LO, 4);

        // Inode 2 (root) at block 4, byte (2-1) * 256 = 256.
        let inode_table_byte = 4 * BLOCK_SIZE;
        let inode2_off = inode_table_byte + 256;
        write_u16(&mut img, inode2_off + I_MODE, root_mode);
        write_u16(&mut img, inode2_off + I_LINKS_COUNT, root_links);

        img
    }

    /// Open succeeds on a valid image and surfaces the superblock.
    ///
    /// Bug it catches: any field-offset slip in superblock decode,
    /// or in the GDT-block computation (`first_data_block + 1`),
    /// would cause `open` to fail or read garbage.
    #[test]
    fn test_open_minimal_image_succeeds() {
        let img = build_image(0o040755, 2);
        let fs = Filesystem::open(Cursor::new(img)).unwrap();
        assert_eq!(fs.superblock().block_size, 4096);
        assert_eq!(fs.superblock().inodes_per_group, 32);
        assert_eq!(fs.superblock().blocks_per_group, 8);
        assert_eq!(fs.group_descriptor_table().len(), 1);
        assert_eq!(fs.group_descriptor_table()[0].inode_table, 4);
    }

    /// Open fails with a typed superblock error on bad magic.
    ///
    /// Bug it catches: a parser that doesn't bubble up the
    /// underlying decode error and instead reports a generic
    /// "open failed" robs callers of the diagnostic. Routing on
    /// `Ext4Error::Superblock(BadMagic)` lets a UI distinguish
    /// "not an ext4 image" from "I/O error".
    #[test]
    fn test_open_bad_magic_returns_superblock_error() {
        let mut img = build_image(0o040755, 2);
        // Corrupt the magic.
        write_u16(&mut img, 1024 + SB_MAGIC, 0xDEAD);
        let err = Filesystem::open(Cursor::new(img)).unwrap_err();
        assert!(matches!(
            err,
            Ext4Error::Superblock(spec::SuperblockDecodeError::BadMagic { found: 0xDEAD })
        ));
    }

    /// Open rejects a corrupt superblock with `blocks_per_group = 0`.
    ///
    /// Bug it catches: a divide-by-zero panic in `group_count()`
    /// would crash an opener faced with this corruption pattern.
    /// Returning a typed error lets the caller report and skip.
    #[test]
    fn test_open_zero_blocks_per_group_returns_invalid_layout() {
        let mut img = build_image(0o040755, 2);
        write_u32(&mut img, 1024 + SB_BLOCKS_PER_GROUP, 0);
        let err = Filesystem::open(Cursor::new(img)).unwrap_err();
        assert!(matches!(err, Ext4Error::InvalidLayout { .. }));
    }

    /// `read_inode(0)` returns InodeOutOfRange.
    ///
    /// Bug it catches: inode 0 is the "no inode" sentinel and
    /// must never be read. A naive `(N - 1) / per_group`
    /// computation on N=0 underflows to u32::MAX, causing a
    /// catastrophic out-of-bounds GDT lookup.
    #[test]
    fn test_read_inode_zero_returns_out_of_range() {
        let img = build_image(0o040755, 2);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let err = fs.read_inode(0).unwrap_err();
        assert!(matches!(err, Ext4Error::InodeOutOfRange { inode: 0, .. }));
    }

    /// `read_inode(N)` with N > inodes_count returns
    /// InodeOutOfRange.
    ///
    /// Bug it catches: a reader that trusts the caller's number
    /// would seek past the inode table into bitmap or data
    /// territory and decode whatever bytes it found as an inode,
    /// returning a "valid" but nonsense record.
    #[test]
    fn test_read_inode_beyond_inodes_count_returns_out_of_range() {
        let img = build_image(0o040755, 2);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let err = fs.read_inode(1_000_000).unwrap_err();
        assert!(matches!(
            err,
            Ext4Error::InodeOutOfRange {
                inode: 1_000_000,
                max: 32
            }
        ));
    }

    /// `read_inode(2)` returns the root inode with the mode bits
    /// the test image was built with.
    ///
    /// Bug it catches: any slip in the inode-location arithmetic
    /// (group, index, table offset, byte offset within table)
    /// would surface as the wrong mode value here. Inode 2 lives
    /// at index 1 within group 0's inode table — the most common
    /// off-by-one path in this kind of code is "(N) / per_group"
    /// instead of "(N - 1) / per_group", which would shift every
    /// inode lookup by one slot.
    #[test]
    fn test_read_inode_2_returns_root_directory_with_expected_mode() {
        let img = build_image(0o040755, 2);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(2).unwrap();
        assert_eq!(inode.mode, 0o040755);
        assert_eq!(inode.file_type(), InodeFileType::Directory);
        assert_eq!(inode.links_count, 2);
        assert!(inode.is_directory());
    }
}

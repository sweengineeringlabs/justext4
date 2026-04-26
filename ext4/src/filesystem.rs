//! Filesystem handle — open an image, read inodes by number.

use std::io::{Read, Seek, SeekFrom};

use spec::{
    decode_extent_node, ExtentNode, GroupDescriptor, Inode, Superblock, SUPERBLOCK_OFFSET,
    SUPERBLOCK_SIZE,
};

use crate::error::Ext4Error;

/// Maximum depth of an ext4 extent tree. The kernel hard-caps tree
/// height; anything deeper is corruption. Bounding the iterative
/// walk by this value guarantees `resolve_logical_block` always
/// terminates regardless of input.
pub const MAX_EXTENT_DEPTH: u16 = 5;

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

    /// Resolve an inode's logical block to its physical block on
    /// disk by walking the extent tree.
    ///
    /// Returns:
    /// - `Ok(Some(physical))` — the logical block is mapped.
    /// - `Ok(None)` — sparse hole (no extent covers this logical
    ///   block) or uninitialised extent (preallocated but not yet
    ///   written; reads as zeros). Higher-level read paths
    ///   zero-fill when they see `None`.
    /// - `Err(NotExtentBased)` — the inode uses the legacy ext2/3
    ///   block-pointer layout, not yet supported.
    /// - `Err(MaxExtentDepthExceeded)` — corrupt image with a
    ///   tree deeper than the kernel's cap of 5 levels.
    ///
    /// Iterative, not recursive. The loop reads at most
    /// `MAX_EXTENT_DEPTH` blocks from disk during descent.
    pub fn resolve_logical_block(
        &mut self,
        inode: &Inode,
        logical: u32,
    ) -> Result<Option<u64>, Ext4Error> {
        if !inode.uses_extents() {
            return Err(Ext4Error::NotExtentBased);
        }

        let block_size = self.superblock.block_size as u64;
        let mut node_buf: Vec<u8> = inode.block.to_vec();
        let mut steps = MAX_EXTENT_DEPTH + 1;

        loop {
            if steps == 0 {
                return Err(Ext4Error::MaxExtentDepthExceeded {
                    max: MAX_EXTENT_DEPTH,
                });
            }
            steps -= 1;

            let node = decode_extent_node(&node_buf)?;
            match node {
                ExtentNode::Leaf { extents, .. } => {
                    for ext in &extents {
                        let start = ext.logical_block;
                        let end = start as u64 + ext.len as u64;
                        if (logical as u64) >= start as u64 && (logical as u64) < end {
                            if ext.uninit {
                                // Uninit extents read as zeros — surfaced as
                                // None so the caller treats them like a hole.
                                return Ok(None);
                            }
                            let offset = (logical - start) as u64;
                            return Ok(Some(ext.physical_block + offset));
                        }
                    }
                    // Logical block falls in a sparse hole between
                    // extents (or past the last extent).
                    return Ok(None);
                }
                ExtentNode::Internal { indices, .. } => {
                    // Pick the rightmost index whose logical_block
                    // is <= the target. Indices are stored sorted
                    // by logical_block per the kernel's invariant.
                    let mut chosen: Option<&spec::ExtentIndex> = None;
                    for idx in &indices {
                        if idx.logical_block <= logical {
                            chosen = Some(idx);
                        } else {
                            break;
                        }
                    }
                    let idx = match chosen {
                        Some(i) => i,
                        // Target is below the first index's start —
                        // sparse hole at the front of the file.
                        None => return Ok(None),
                    };

                    let leaf_offset = idx.leaf_block * block_size;
                    self.reader.seek(SeekFrom::Start(leaf_offset))?;
                    node_buf = vec![0u8; block_size as usize];
                    self.reader.read_exact(&mut node_buf)?;
                }
            }
        }
    }

    /// Read the full contents of a regular file or symlink into a
    /// `Vec<u8>`. Walks the extent tree for each logical block,
    /// reads `block_size` bytes per mapped block, zero-fills for
    /// holes and uninit extents, and truncates the result to
    /// `inode.size`.
    ///
    /// Caller is expected to inspect the inode's file type before
    /// calling — this method does not check `is_regular()` because
    /// symlink targets and inline-data inodes both want byte
    /// contents too. Calling on a directory inode is allowed and
    /// returns the raw concatenated directory blocks; consumers
    /// that want entries should use a directory-specific method.
    ///
    /// Inline-data inodes (`INODE_FLAG_INLINE_DATA`) are not yet
    /// supported. v0 of read_file treats them as having a size-0
    /// extent tree and returns whatever the empty walk yields,
    /// which is wrong for the small handful of files that use
    /// inline-data; document and address in a follow-up slice.
    pub fn read_file(&mut self, inode: &Inode) -> Result<Vec<u8>, Ext4Error> {
        let size = inode.size as usize;
        if size == 0 {
            return Ok(Vec::new());
        }

        let block_size = self.superblock.block_size as usize;
        let num_blocks = size.div_ceil(block_size);

        let mut data = Vec::with_capacity(num_blocks * block_size);
        for logical in 0..num_blocks {
            // Cast is fine — file size is bounded by u64; even at
            // 16 TiB with 4 KiB blocks, num_blocks fits in u32.
            // Defensive on the cast in case a malformed image
            // claims an absurd size.
            let logical_u32: u32 = logical.try_into().map_err(|_| Ext4Error::InvalidLayout {
                reason: "file logical block index exceeds u32",
            })?;

            match self.resolve_logical_block(inode, logical_u32)? {
                Some(physical) => {
                    let offset = physical * (block_size as u64);
                    self.reader.seek(SeekFrom::Start(offset))?;
                    let start = data.len();
                    data.resize(start + block_size, 0);
                    self.reader.read_exact(&mut data[start..])?;
                }
                None => {
                    // Hole or uninit — zero-fill in place.
                    data.resize(data.len() + block_size, 0);
                }
            }
        }

        data.truncate(size);
        Ok(data)
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
    // Bumped from 8 to 16 (64 KiB total) to leave room for extent
    // tree leaf blocks during depth>0 walk tests.
    const NUM_BLOCKS: u32 = 16;
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
    const I_SIZE_LO: usize = 0x04;
    const I_LINKS_COUNT: usize = 0x1A;
    const I_FLAGS: usize = 0x20;
    const I_BLOCK: usize = 0x28;

    // Extent tree on-disk constants (mirror spec::extent).
    const EXTENT_MAGIC: u16 = 0xF30A;
    const EXTENT_FLAG: u32 = 0x0008_0000; // INODE_FLAG_EXTENTS

    fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Byte offset of inode N within the test image (computed
    /// from the layout build_image() produces).
    fn inode_byte_offset(inode_number: u32) -> usize {
        // Inode table starts at block 4. Inode N lives at index
        // (N - 1) within that table.
        4 * BLOCK_SIZE + (inode_number as usize - 1) * INODE_SIZE as usize
    }

    /// Write a 12-byte extent header at `byte_offset`.
    fn write_extent_header(buf: &mut [u8], byte_offset: usize, entries: u16, max: u16, depth: u16) {
        write_u16(buf, byte_offset, EXTENT_MAGIC);
        write_u16(buf, byte_offset + 2, entries);
        write_u16(buf, byte_offset + 4, max);
        write_u16(buf, byte_offset + 6, depth);
        write_u32(buf, byte_offset + 8, 0); // generation
    }

    /// Write a 12-byte leaf extent entry at `byte_offset`.
    /// `len_raw` is encoded directly (caller may add 32768 to
    /// signal uninit per the kernel's convention).
    fn write_extent_leaf(
        buf: &mut [u8],
        byte_offset: usize,
        logical: u32,
        len_raw: u16,
        physical: u64,
    ) {
        write_u32(buf, byte_offset, logical);
        write_u16(buf, byte_offset + 4, len_raw);
        write_u16(buf, byte_offset + 6, ((physical >> 32) & 0xFFFF) as u16);
        write_u32(buf, byte_offset + 8, (physical & 0xFFFF_FFFF) as u32);
    }

    /// Write a 12-byte extent index entry at `byte_offset`.
    fn write_extent_index(buf: &mut [u8], byte_offset: usize, logical: u32, leaf_block: u64) {
        write_u32(buf, byte_offset, logical);
        write_u32(buf, byte_offset + 4, (leaf_block & 0xFFFF_FFFF) as u32);
        write_u16(buf, byte_offset + 8, ((leaf_block >> 32) & 0xFFFF) as u16);
        write_u16(buf, byte_offset + 10, 0); // ei_unused
    }

    /// Stamp inode N as an extents-based regular file with one
    /// leaf extent in i_block. Returns nothing; caller can read
    /// the inode via Filesystem::read_inode after building.
    ///
    /// Pass `len_raw = len + 32768` to test the uninit path.
    fn write_inode_with_one_extent(
        buf: &mut [u8],
        inode_number: u32,
        size: u32,
        logical: u32,
        len_raw: u16,
        physical: u64,
    ) {
        let inode_off = inode_byte_offset(inode_number);
        write_u16(buf, inode_off + I_MODE, 0o100644); // S_IFREG | 0o644
        write_u32(buf, inode_off + I_SIZE_LO, size);
        write_u16(buf, inode_off + I_LINKS_COUNT, 1);
        write_u32(buf, inode_off + I_FLAGS, EXTENT_FLAG);
        write_extent_header(buf, inode_off + I_BLOCK, 1, 4, 0);
        write_extent_leaf(buf, inode_off + I_BLOCK + 12, logical, len_raw, physical);
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
        assert_eq!(fs.superblock().inodes_per_group, INODES_PER_GROUP);
        assert_eq!(fs.superblock().blocks_per_group, NUM_BLOCKS);
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

    /// `resolve_logical_block` rejects inodes that don't have the
    /// EXTENTS flag with a typed error.
    ///
    /// Bug it catches: silently treating a legacy block-pointer
    /// inode as if its `i_block` were an extent header would
    /// either fail with a confusing "bad magic" message (the first
    /// 2 bytes of `i_block` are u32 block pointer, not 0xF30A) or,
    /// worse, decode the pointer as a valid header by chance and
    /// produce nonsense mappings. The typed error tells the
    /// caller which surface they actually need.
    #[test]
    fn test_resolve_legacy_block_pointer_inode_returns_not_extent_based() {
        let img = build_image(0o040755, 2);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        // Root inode (inode 2) was built without the EXTENTS flag.
        let inode = fs.read_inode(2).unwrap();
        let err = fs.resolve_logical_block(&inode, 0).unwrap_err();
        assert!(matches!(err, Ext4Error::NotExtentBased));
    }

    /// Single leaf extent: logical 0..8 maps to physical 100..108.
    /// resolve(3) returns Some(103); resolve(8) returns None
    /// (sparse hole past the end of the only extent).
    ///
    /// Bug it catches: a walker that returns ext.physical_block
    /// without adding `(logical - ext.logical_block)` would always
    /// return the start of the extent, sending every read of the
    /// file's interior blocks to its first physical block. File
    /// content would look like the same 4 KiB chunk repeated.
    #[test]
    fn test_resolve_single_leaf_extent_offsets_correctly() {
        let mut img = build_image(0o040755, 2);
        // Inode 11: extent-based file; logical 0..8 → physical 100..108.
        write_inode_with_one_extent(&mut img, 11, 32_768, 0, 8, 100);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();

        assert_eq!(fs.resolve_logical_block(&inode, 0).unwrap(), Some(100));
        assert_eq!(fs.resolve_logical_block(&inode, 3).unwrap(), Some(103));
        assert_eq!(fs.resolve_logical_block(&inode, 7).unwrap(), Some(107));
        assert_eq!(fs.resolve_logical_block(&inode, 8).unwrap(), None);
    }

    /// Uninit extent (raw len > 32768) resolves to None — the
    /// caller treats it as a sparse hole and zero-fills.
    ///
    /// Bug it catches: a walker that returns the physical address
    /// of an uninit extent would have higher layers read whatever
    /// stale bytes happen to live in those preallocated blocks,
    /// leaking data from a previous file (or, on disks that
    /// haven't been zeroed, arbitrary content from prior owners)
    /// into the current file's reads.
    #[test]
    fn test_resolve_uninit_extent_returns_none() {
        let mut img = build_image(0o040755, 2);
        // raw_len = 32768 + 4 → uninit, real run length 4.
        write_inode_with_one_extent(&mut img, 11, 16_384, 0, 32768 + 4, 100);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        // Within the run; uninit means None.
        assert_eq!(fs.resolve_logical_block(&inode, 0).unwrap(), None);
        assert_eq!(fs.resolve_logical_block(&inode, 3).unwrap(), None);
    }

    /// Sparse hole between extents resolves to None.
    ///
    /// Bug it catches: a walker that returns the *next* extent's
    /// physical address for a hole between extents would conflate
    /// holes with the start of the following data. Reading a
    /// sparse file would shift content forward by the size of
    /// every preceding hole.
    #[test]
    fn test_resolve_sparse_hole_between_extents_returns_none() {
        let mut img = build_image(0o040755, 2);
        // Build manually: two extents in i_block — logical 0..4 →
        // physical 100, then logical 100..104 → physical 200.
        // Logical 50 is in the hole.
        let inode_off = inode_byte_offset(11);
        write_u16(&mut img, inode_off + I_MODE, 0o100644);
        write_u32(&mut img, inode_off + I_SIZE_LO, 4096 * 104);
        write_u16(&mut img, inode_off + I_LINKS_COUNT, 1);
        write_u32(&mut img, inode_off + I_FLAGS, EXTENT_FLAG);
        write_extent_header(&mut img, inode_off + I_BLOCK, 2, 4, 0);
        write_extent_leaf(&mut img, inode_off + I_BLOCK + 12, 0, 4, 100);
        write_extent_leaf(&mut img, inode_off + I_BLOCK + 24, 100, 4, 200);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        assert_eq!(fs.resolve_logical_block(&inode, 0).unwrap(), Some(100));
        assert_eq!(fs.resolve_logical_block(&inode, 50).unwrap(), None); // hole
        assert_eq!(fs.resolve_logical_block(&inode, 100).unwrap(), Some(200));
    }

    /// Depth=1 internal node descends to a leaf block on disk and
    /// resolves correctly.
    ///
    /// Bug it catches: any error in the iterative descent — wrong
    /// index choice, wrong leaf-block address arithmetic, failure
    /// to re-decode the leaf block as a fresh node — would break
    /// every file with more than 4 extents (the inode-embedded
    /// header maxes at 4; anything bigger needs depth>0). The
    /// test pins the descent end-to-end.
    #[test]
    fn test_resolve_depth_1_descends_to_leaf_block() {
        let mut img = build_image(0o040755, 2);
        let inode_off = inode_byte_offset(11);

        // Inode 11: i_block = depth=1 internal node. One index
        // entry at logical 0 pointing to leaf at block 8.
        write_u16(&mut img, inode_off + I_MODE, 0o100644);
        write_u32(&mut img, inode_off + I_SIZE_LO, 4096);
        write_u16(&mut img, inode_off + I_LINKS_COUNT, 1);
        write_u32(&mut img, inode_off + I_FLAGS, EXTENT_FLAG);
        write_extent_header(&mut img, inode_off + I_BLOCK, 1, 4, 1);
        write_extent_index(&mut img, inode_off + I_BLOCK + 12, 0, 8);

        // Leaf node at block 8: depth=0 header + one extent
        // covering logical 0..1 → physical 200.
        let leaf_off = 8 * BLOCK_SIZE;
        write_extent_header(&mut img, leaf_off, 1, 340, 0);
        write_extent_leaf(&mut img, leaf_off + 12, 0, 1, 200);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        assert_eq!(fs.resolve_logical_block(&inode, 0).unwrap(), Some(200));
    }

    /// Write `data` into a physical block region of the image,
    /// starting at `physical_block * BLOCK_SIZE`. Used by
    /// read_file tests to populate the disk side of the extent
    /// mapping.
    fn write_data_at_block(buf: &mut [u8], physical_block: u64, data: &[u8]) {
        let off = physical_block as usize * BLOCK_SIZE;
        buf[off..off + data.len()].copy_from_slice(data);
    }

    /// `read_file` on a zero-size inode returns an empty Vec, no
    /// IO performed.
    ///
    /// Bug it catches: a reader that processes the first block
    /// regardless of size would either OOB-read past the inode's
    /// extents (returning garbage) or call resolve on logical=0
    /// of an inode that has no extents (succeeding then returning
    /// 4 KiB of zeros, which would be wrong because the file's
    /// declared size is 0).
    #[test]
    fn test_read_file_zero_size_returns_empty_vec() {
        let mut img = build_image(0o040755, 2);
        write_inode_with_one_extent(&mut img, 11, 0, 0, 1, 6);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();
        assert!(data.is_empty());
    }

    /// File fitting in one block returns exactly the bytes that
    /// were placed at the mapped physical block, truncated to
    /// `inode.size`.
    ///
    /// Bug it catches: a reader that returns a full `block_size`
    /// blob without truncating to `inode.size` would include
    /// trailing padding bytes (`block_size - size` bytes of
    /// whatever happens to be in the data block past the file's
    /// real end). On a freshly-zeroed image this would be zeros;
    /// on a reused block it would be the previous tenant's data,
    /// a real information-leak bug.
    #[test]
    fn test_read_file_small_payload_fits_in_one_block() {
        let payload = b"hello, ext4!";
        let mut img = build_image(0o040755, 2);
        write_inode_with_one_extent(&mut img, 11, payload.len() as u32, 0, 1, 6);
        write_data_at_block(&mut img, 6, payload);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();
        assert_eq!(data, payload);
    }

    /// File spanning two blocks reads the second block too —
    /// concatenation matches the on-disk layout.
    ///
    /// Bug it catches: a reader that stops after the first block
    /// (e.g. returning early when the first read fills more than
    /// inode.size) would silently truncate any file larger than
    /// `block_size`, returning only its first 4 KiB. The test
    /// puts distinguishable bytes in each block so a one-block
    /// truncation surfaces immediately.
    #[test]
    fn test_read_file_two_block_payload_concatenates() {
        let mut img = build_image(0o040755, 2);
        let block_a: Vec<u8> = (0..BLOCK_SIZE).map(|i| (i & 0xFF) as u8).collect();
        let block_b: Vec<u8> = (0..BLOCK_SIZE).map(|i| ((i + 1) & 0xFF) as u8).collect();
        let total_size = (BLOCK_SIZE * 2) as u32;
        // Extent: logical 0..2 → physical 6..8.
        write_inode_with_one_extent(&mut img, 11, total_size, 0, 2, 6);
        write_data_at_block(&mut img, 6, &block_a);
        write_data_at_block(&mut img, 7, &block_b);
        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();
        let mut expected = block_a.clone();
        expected.extend_from_slice(&block_b);
        assert_eq!(data, expected);
    }

    /// Sparse hole zero-fills in the middle of the read.
    ///
    /// Bug it catches: a reader that propagates a None resolve
    /// as an error would refuse to read sparse files at all —
    /// many production images use sparse files for log rotation
    /// and disk-image storage. The test interleaves real data
    /// with a hole and checks the hole region is zeros while
    /// data on either side matches.
    #[test]
    fn test_read_file_sparse_hole_zero_fills_middle() {
        let mut img = build_image(0o040755, 2);
        let inode_off = inode_byte_offset(11);
        let total_size = (BLOCK_SIZE * 3) as u32;
        write_u16(&mut img, inode_off + I_MODE, 0o100644);
        write_u32(&mut img, inode_off + I_SIZE_LO, total_size);
        write_u16(&mut img, inode_off + I_LINKS_COUNT, 1);
        write_u32(&mut img, inode_off + I_FLAGS, EXTENT_FLAG);
        write_extent_header(&mut img, inode_off + I_BLOCK, 2, 4, 0);
        // Logical 0..1 → physical 6.
        write_extent_leaf(&mut img, inode_off + I_BLOCK + 12, 0, 1, 6);
        // Logical 2..3 → physical 7. Logical 1 is a hole.
        write_extent_leaf(&mut img, inode_off + I_BLOCK + 24, 2, 1, 7);

        let block_a = vec![0xAAu8; BLOCK_SIZE];
        let block_c = vec![0xCCu8; BLOCK_SIZE];
        write_data_at_block(&mut img, 6, &block_a);
        write_data_at_block(&mut img, 7, &block_c);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();

        assert_eq!(&data[..BLOCK_SIZE], &block_a[..]);
        assert!(data[BLOCK_SIZE..2 * BLOCK_SIZE].iter().all(|&b| b == 0));
        assert_eq!(&data[2 * BLOCK_SIZE..3 * BLOCK_SIZE], &block_c[..]);
    }

    /// Uninit extent (preallocated, never written) reads as
    /// all zeros.
    ///
    /// Bug it catches: a reader that returns the actual on-disk
    /// bytes from a preallocated block would surface stale
    /// content from a previous tenant of those blocks. The
    /// physical block holds 0xFF in this test; correct behaviour
    /// is to return zeros.
    #[test]
    fn test_read_file_uninit_extent_reads_as_zeros() {
        let mut img = build_image(0o040755, 2);
        // raw_len = 32768 + 1 → uninit, real run length 1.
        write_inode_with_one_extent(&mut img, 11, BLOCK_SIZE as u32, 0, 32768 + 1, 6);
        // Pollute physical block 6 with non-zero bytes that would
        // surface if the reader didn't honour the uninit flag.
        let stale = vec![0xFFu8; BLOCK_SIZE];
        write_data_at_block(&mut img, 6, &stale);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();

        assert_eq!(data.len(), BLOCK_SIZE);
        assert!(data.iter().all(|&b| b == 0));
    }

    /// Read truncates to `inode.size` even when the last block is
    /// not fully consumed.
    ///
    /// Bug it catches: a reader that emits whole blocks without
    /// truncation would leak (block_size - size % block_size)
    /// trailing bytes past the file's real end. On reused blocks
    /// those bytes are a previous tenant's data. The test sets
    /// inode.size = block_size + 5 and seeds the second block
    /// with sentinel bytes past byte 5, asserting the read stops
    /// at byte 5 of block 2 (total length = block_size + 5).
    #[test]
    fn test_read_file_truncates_to_inode_size_in_last_block() {
        let mut img = build_image(0o040755, 2);
        let total_size = (BLOCK_SIZE + 5) as u32;
        write_inode_with_one_extent(&mut img, 11, total_size, 0, 2, 6);

        let block_a = vec![0xAAu8; BLOCK_SIZE];
        let mut block_b = vec![0xFFu8; BLOCK_SIZE];
        block_b[0..5].copy_from_slice(b"abcde");
        write_data_at_block(&mut img, 6, &block_a);
        write_data_at_block(&mut img, 7, &block_b);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        let data = fs.read_file(&inode).unwrap();

        assert_eq!(data.len(), BLOCK_SIZE + 5);
        assert_eq!(&data[..BLOCK_SIZE], &block_a[..]);
        assert_eq!(&data[BLOCK_SIZE..], b"abcde");
    }

    /// An empty extent tree (entries=0 in i_block) resolves
    /// every logical block to None.
    ///
    /// Bug it catches: a walker that reads past the end of a
    /// zero-entry extent body would either OOB-read i_block or
    /// invent a garbage extent from the next 12 zero bytes
    /// (logical=0, len=0, physical=0). With len=0, every logical
    /// block falsely "matches" the extent's [0, 0) range — luckily
    /// `<` makes this empty, but a parser using `<=` would map
    /// every read to physical block 0 (the partition's MBR-
    /// equivalent).
    #[test]
    fn test_resolve_empty_extent_tree_returns_none_for_any_logical() {
        let mut img = build_image(0o040755, 2);
        let inode_off = inode_byte_offset(11);
        write_u16(&mut img, inode_off + I_MODE, 0o100644);
        write_u32(&mut img, inode_off + I_FLAGS, EXTENT_FLAG);
        write_extent_header(&mut img, inode_off + I_BLOCK, 0, 4, 0);

        let mut fs = Filesystem::open(Cursor::new(img)).unwrap();
        let inode = fs.read_inode(11).unwrap();
        assert_eq!(fs.resolve_logical_block(&inode, 0).unwrap(), None);
        assert_eq!(fs.resolve_logical_block(&inode, 999).unwrap(), None);
    }
}

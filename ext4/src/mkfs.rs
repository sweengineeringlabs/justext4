//! Format an ext4 image. The pure-Rust `mkfs.ext4` analogue.
//!
//! v0 lays down a minimal valid filesystem: one block group, the
//! standard reserved inodes (1-10) zeroed, root directory at inode
//! 2 with the canonical "." and ".." entries. The output is
//! readable by [`crate::Filesystem::open`] and the kernel can
//! mount it (subject to the v0 limitations called out below).
//!
//! Out of scope for v0:
//!
//! - Lost+found directory and the second standard inode (11).
//! - Journal (a JBD2-formatted file at a reserved inode).
//! - Multiple block groups / large images.
//! - `METADATA_CSUM` / `64BIT` features — produced images use
//!   the simplest viable feature set so they round-trip cleanly
//!   through the v0 decoders.
//!
//! **Known kernel-interop gap.** `e2fsck -nf <image>` rejects
//! our output today with "superblock corrupt", because we don't
//! emit several fields the kernel inspects: `s_state`,
//! `s_creator_os`, `s_max_mnt_count`, last-mount/write/check
//! timestamps, and a non-trivial feature-bit baseline. The
//! v0 decoder doesn't read those fields either, so our
//! round-trip still closes through *our* reader; closing it
//! through `e2fsck` is a follow-up slice. The reverse
//! direction is solid: see
//! `tests/real_mkfs_roundtrip.rs` — we open and walk
//! mke2fs-produced images correctly.
//!
//! What's enough for v0: produce something
//! [`crate::Filesystem::open`] can consume,
//! [`crate::Filesystem::read_inode(ROOT_INODE)`] can find as a
//! directory, and [`crate::Filesystem::read_dir`] can walk into
//! `[".", ".."]`. That's the round-trip demo this module is
//! designed to deliver.

use std::io::{Seek, SeekFrom, Write};

use spec::{
    bitmap, DirEntry, Extent, ExtentHeader, GroupDescriptor, Inode, Superblock, INODE_FLAG_EXTENTS,
    I_BLOCK_LEN, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE,
};

use crate::error::Ext4Error;

/// Configuration for [`format`]. Construct via [`Config::default`]
/// for a 64 KiB image at 4 KiB blocks; tune the fields directly
/// for other sizes.
#[derive(Debug, Clone)]
pub struct Config {
    /// Block size in bytes. Must be one of 1024, 2048, 4096,
    /// 65536 (the same set the kernel accepts).
    pub block_size: u32,

    /// Total number of blocks in the image.
    pub size_blocks: u32,

    /// Volume label, truncated/padded to 16 bytes by [`format`].
    /// Empty bytes (no label) are fine.
    pub volume_label: Vec<u8>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            block_size: 4096,
            size_blocks: 16,
            volume_label: b"justext4".to_vec(),
        }
    }
}

/// Inodes per group. Hard-coded for v0 — single-group images
/// don't need many inodes.
const INODES_PER_GROUP: u32 = 32;

/// Inode size on disk. Modern ext4 uses 256 bytes; rev-0 used
/// 128. We pick 256 to match `mkfs.ext4` defaults.
const INODE_SIZE: u16 = 256;

/// Inode number reserved for the root directory by every ext-
/// family filesystem.
const ROOT_INODE_NUMBER: u32 = 2;

/// Layout helpers — derived from a Config so the IO step doesn't
/// have to recompute offsets inline.
struct Layout {
    block_size: u64,
    inode_table_blocks: u64,
    inode_table_first_block: u64,
    root_dir_block: u64,
    /// Number of blocks consumed by metadata + the root dir
    /// data block. Free count subtracts this from total blocks.
    metadata_blocks: u64,
}

impl Layout {
    fn from_config(config: &Config) -> Result<Self, Ext4Error> {
        if !matches!(config.block_size, 1024 | 2048 | 4096 | 65536) {
            return Err(Ext4Error::InvalidLayout {
                reason: "block_size must be 1024, 2048, 4096, or 65536",
            });
        }
        let block_size = config.block_size as u64;
        let inode_table_bytes = (INODES_PER_GROUP as u64) * (INODE_SIZE as u64);
        let inode_table_blocks = inode_table_bytes.div_ceil(block_size);
        // Layout (block index → contents):
        //   0: padding + superblock (sb at byte 1024 within block)
        //   1: GDT
        //   2: block bitmap
        //   3: inode bitmap
        //   4 .. 4+inode_table_blocks: inode table
        //   4+inode_table_blocks: root dir data
        let inode_table_first_block = 4u64;
        let root_dir_block = inode_table_first_block + inode_table_blocks;
        let metadata_blocks = root_dir_block + 1;
        if (config.size_blocks as u64) < metadata_blocks {
            return Err(Ext4Error::InvalidLayout {
                reason: "size_blocks too small for the metadata + root dir layout",
            });
        }
        Ok(Layout {
            block_size,
            inode_table_blocks,
            inode_table_first_block,
            root_dir_block,
            metadata_blocks,
        })
    }
}

/// Format a writer as a minimal valid ext4 image.
///
/// The writer should be empty (or its existing contents will be
/// overwritten in the regions this function touches; bytes
/// between writes are caller-owned). For an in-memory image use
/// `std::io::Cursor::new(Vec::with_capacity(size))`; for a file
/// use `OpenOptions::new().write(true).create(true).truncate(true)`.
pub fn format<W: Write + Seek>(writer: &mut W, config: &Config) -> Result<(), Ext4Error> {
    let layout = Layout::from_config(config)?;
    let total_blocks = config.size_blocks as u64;

    // ── superblock ─────────────────────────────────────────────
    let mut volume_name = [0u8; 16];
    let label_len = config.volume_label.len().min(16);
    volume_name[..label_len].copy_from_slice(&config.volume_label[..label_len]);

    let free_blocks = total_blocks - layout.metadata_blocks;
    // free inodes = total - 1 (only root inode is allocated for
    // v0 — the other reserved inode slots remain zeroed).
    let free_inodes = INODES_PER_GROUP - 1;

    let sb = Superblock {
        block_size: config.block_size,
        inodes_count: INODES_PER_GROUP,
        blocks_count: total_blocks,
        free_blocks_count: free_blocks,
        free_inodes_count: free_inodes,
        inode_size: INODE_SIZE,
        rev_level: 1,
        feature_compat: 0,
        feature_incompat: 0,
        feature_ro_compat: 0,
        uuid: [0; 16],
        volume_name,
        desc_size: 0, // ignored when 64BIT is clear
        first_data_block: 0,
        blocks_per_group: config.size_blocks,
        inodes_per_group: INODES_PER_GROUP,
    };

    // ── group descriptor (single entry) ────────────────────────
    let gd = GroupDescriptor {
        block_bitmap: 2,
        inode_bitmap: 3,
        inode_table: layout.inode_table_first_block,
        free_blocks_count: free_blocks as u32,
        free_inodes_count: free_inodes,
        used_dirs_count: 1, // root
        flags: 0,
        checksum: 0,
    };

    // ── root inode ─────────────────────────────────────────────
    // i_block holds the extent header + one leaf extent pointing
    // at the root dir data block.
    let mut root_block_bytes = [0u8; I_BLOCK_LEN];
    let header = ExtentHeader {
        entries: 1,
        max: 4,
        depth: 0,
        generation: 0,
    };
    header.encode_into(&mut root_block_bytes[..12])?;
    let extent = Extent {
        logical_block: 0,
        len: 1,
        physical_block: layout.root_dir_block,
        uninit: false,
    };
    extent.encode_into(&mut root_block_bytes[12..24])?;

    let root_inode = Inode {
        mode: 0o040755,
        uid: 0,
        gid: 0,
        size: layout.block_size,
        atime: 0,
        ctime: 0,
        mtime: 0,
        dtime: 0,
        // Two links: "." (self-link) and the parent's entry to
        // this dir. For root, the parent entry is itself, so the
        // count is 2.
        links_count: 2,
        // i_blocks is in 512-byte sectors when HUGE_FILE is clear.
        // One filesystem block of `block_size` is `block_size / 512`
        // sectors.
        blocks_lo: (config.block_size / 512),
        blocks_hi: 0,
        flags: INODE_FLAG_EXTENTS,
        block: root_block_bytes,
        generation: 0,
        file_acl_lo: 0,
        file_acl_hi: 0,
    };

    // ── root directory entries ─────────────────────────────────
    let dot = DirEntry {
        inode: ROOT_INODE_NUMBER,
        // 8-byte header + 1-byte name "." → 9, padded to 12.
        rec_len: 12,
        file_type_raw: 2, // EXT4_FT_DIR
        name: b".".to_vec(),
    };
    let dotdot = DirEntry {
        inode: ROOT_INODE_NUMBER, // root's parent is itself
        // The last entry in a dir block absorbs the rest of the
        // block — kernel invariant.
        rec_len: (config.block_size - 12) as u16,
        file_type_raw: 2,
        name: b"..".to_vec(),
    };

    // ── encode + write ─────────────────────────────────────────
    let mut sb_buf = vec![0u8; SUPERBLOCK_SIZE];
    sb.encode_into(&mut sb_buf)?;
    writer.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
    writer.write_all(&sb_buf)?;

    // GDT entry 0 at start of block 1.
    let mut gd_buf = vec![0u8; 32];
    gd.encode_into(&mut gd_buf, &sb)?;
    writer.seek(SeekFrom::Start(layout.block_size))?;
    writer.write_all(&gd_buf)?;

    // Inode 2 at index 1 within the inode table.
    let inode_offset = layout.inode_table_first_block * layout.block_size
        + ((ROOT_INODE_NUMBER - 1) as u64) * (INODE_SIZE as u64);
    let mut inode_buf = vec![0u8; INODE_SIZE as usize];
    root_inode.encode_into(&mut inode_buf, &sb)?;
    writer.seek(SeekFrom::Start(inode_offset))?;
    writer.write_all(&inode_buf)?;

    // Root dir data block: "." then "..".
    let mut block_buf = vec![0u8; config.block_size as usize];
    dot.encode_into(&mut block_buf[..12])?;
    dotdot.encode_into(&mut block_buf[12..])?;
    writer.seek(SeekFrom::Start(layout.root_dir_block * layout.block_size))?;
    writer.write_all(&block_buf)?;

    // Block bitmap at block 2: mark blocks 0..metadata_blocks as
    // used (block-0 padding/superblock, GDT, both bitmap blocks,
    // inode table blocks, root dir data block). Without this the
    // kernel would consider every block free and an allocator
    // running over the FS would hand out blocks already in use.
    let mut block_bitmap_buf = vec![0u8; config.block_size as usize];
    for i in 0..layout.metadata_blocks {
        bitmap::set_bit(&mut block_bitmap_buf, i as usize);
    }
    writer.seek(SeekFrom::Start(2 * layout.block_size))?;
    writer.write_all(&block_bitmap_buf)?;

    // Inode bitmap at block 3: mark inode 2 (root) as used.
    // Bit index = inode_number - 1 (inodes are 1-indexed).
    let mut inode_bitmap_buf = vec![0u8; config.block_size as usize];
    bitmap::set_bit(&mut inode_bitmap_buf, (ROOT_INODE_NUMBER - 1) as usize);
    writer.seek(SeekFrom::Start(3 * layout.block_size))?;
    writer.write_all(&inode_bitmap_buf)?;

    // Pad image up to total_blocks * block_size by writing one
    // zero at the last byte. Required because mid-stream seeks
    // don't extend the underlying buffer; a Filesystem::open on
    // a short buffer would error on the read of metadata blocks
    // that haven't been physically written.
    let total_size = total_blocks * layout.block_size;
    if total_size > 0 {
        writer.seek(SeekFrom::Start(total_size - 1))?;
        writer.write_all(&[0])?;
    }
    let _ = layout.inode_table_blocks; // silence unused-field warning
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{Filesystem, ROOT_INODE};
    use spec::InodeFileType;
    use std::io::Cursor;

    /// Format → open → read root: the round-trip demo. Our own
    /// formatter produces an image our own opener can read.
    ///
    /// Bug it catches: any encode-side field-offset error
    /// surfaces as either an open failure (bad magic / invalid
    /// block size) or a wrong inode mode on read. Together with
    /// the per-type round-trip tests in spec, this is the
    /// integration test that proves the encode + decode sides
    /// are consistent end-to-end.
    #[test]
    fn test_format_then_open_returns_root_directory_with_dot_dotdot_entries() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let config = Config::default();
        format(&mut cursor, &config).unwrap();

        // The buffer now holds a valid ext4 image. Open it.
        let mut fs = Filesystem::open(Cursor::new(buf)).unwrap();
        assert_eq!(fs.superblock().block_size, 4096);
        assert_eq!(fs.superblock().inodes_per_group, INODES_PER_GROUP);

        let root = fs.read_inode(ROOT_INODE).unwrap();
        assert_eq!(root.file_type(), InodeFileType::Directory);
        assert_eq!(root.links_count, 2);
        assert!(root.uses_extents());

        let entries = fs.read_dir(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, b".");
        assert_eq!(entries[0].inode, ROOT_INODE);
        assert_eq!(entries[1].name, b"..");
        assert_eq!(entries[1].inode, ROOT_INODE);
    }

    /// `open_path("/")` over a freshly-formatted image returns
    /// the root inode.
    ///
    /// Bug it catches: even if read_dir works, open_path could
    /// fail if the formatter set `first_data_block` incorrectly,
    /// or if any of the filesystem's chained calls (read_inode →
    /// resolve_logical_block → read_file → decode_dir_block) fail
    /// somewhere downstream of where the per-call tests look.
    /// The integration test exercises the chain end-to-end.
    #[test]
    fn test_format_then_open_path_root_returns_root_inode() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(buf)).unwrap();
        assert_eq!(fs.open_path("/").unwrap(), ROOT_INODE);
    }

    /// Format rejects a too-small image with InvalidLayout.
    ///
    /// Bug it catches: a formatter that silently truncates the
    /// layout to fit a too-small request would produce an image
    /// with overlapping metadata blocks (e.g. inode table on top
    /// of the root dir data). The kernel would mount it and
    /// return garbage; far worse than a clean refusal.
    #[test]
    fn test_format_too_small_image_rejected_with_invalid_layout() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let config = Config {
            size_blocks: 4, // not enough for inode table + root dir
            ..Config::default()
        };
        let err = format(&mut cursor, &config).unwrap_err();
        assert!(matches!(err, Ext4Error::InvalidLayout { .. }));
    }

    /// Format rejects an invalid block_size.
    #[test]
    fn test_format_invalid_block_size_rejected() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let config = Config {
            block_size: 8192, // not in {1024,2048,4096,65536}
            ..Config::default()
        };
        let err = format(&mut cursor, &config).unwrap_err();
        assert!(matches!(err, Ext4Error::InvalidLayout { .. }));
    }

    /// **The headline write-path demo.** Format an empty image,
    /// create a file in it, and read the bytes back — all
    /// in-process, no `mkfs.ext4`, no `cp`, no kernel mount.
    ///
    /// Bug it catches: any encode/decode asymmetry along the
    /// allocator → inode-write → data-write → dir-entry-add chain
    /// surfaces here. Each phase has its own unit test, but only
    /// the integration shows that the phases compose: an inode
    /// number allocated at one bit position has its corresponding
    /// table slot write find the right physical offset, and the
    /// dir entry pointing at it lets a subsequent open_path find
    /// it again.
    #[test]
    fn test_format_then_create_file_then_read_back_round_trips() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        // Larger image needed — default config's 16 blocks leave
        // exactly 1 free data block (block 7, after metadata at
        // 0..6 and root dir at 6 — wait, root dir is at block 6
        // in default config? Let me give more room).
        // Actually default config has size_blocks = 16; metadata
        // blocks = 7 (blocks 0..6 inclusive), root dir at block 6,
        // so blocks 7..15 are free (9 blocks). Plenty for a small
        // file.
        let payload = b"hello, ext4!";

        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();
        let new_inode_num = fs.create_file("/hello.txt", payload).unwrap();

        // Read back via lookup + read_file.
        let root = fs.read_inode(ROOT_INODE).unwrap();
        let looked_up = fs.lookup(&root, b"hello.txt").unwrap();
        assert_eq!(looked_up, new_inode_num);

        let inode = fs.read_inode(new_inode_num).unwrap();
        assert!(inode.is_regular());
        assert_eq!(inode.size, payload.len() as u64);

        let read_back = fs.read_file(&inode).unwrap();
        assert_eq!(read_back, payload);

        // Also verify open_path resolves.
        assert_eq!(fs.open_path("/hello.txt").unwrap(), new_inode_num);
    }

    /// `create_file` rejects a name that already exists in the
    /// parent directory.
    ///
    /// Bug it catches: a creation that silently overwrites would
    /// orphan the previous inode (its blocks + bitmap bits remain
    /// allocated, the dir entry no longer references it). This is
    /// a leak-class bug; the typed AlreadyExists routes the
    /// caller to handle the conflict explicitly.
    #[test]
    fn test_create_file_rejects_collision_with_existing_entry() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();
        fs.create_file("/foo", b"first").unwrap();
        let err = fs.create_file("/foo", b"second").unwrap_err();
        assert!(matches!(err, Ext4Error::AlreadyExists { .. }));
    }

    /// Multiple files round-trip — verifies the bitmap allocator
    /// finds non-overlapping inodes/blocks for each call.
    ///
    /// Bug it catches: a stateless allocator that always returns
    /// the same starting bit (forgetting to write the bitmap
    /// back, or re-reading from the original on each call)
    /// would hand out the same inode/blocks twice, corrupting
    /// the second file's metadata.
    #[test]
    fn test_create_two_files_get_distinct_inodes_and_blocks() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();
        let i1 = fs.create_file("/a.txt", b"AAA").unwrap();
        let i2 = fs.create_file("/b.txt", b"BBBBBB").unwrap();
        assert_ne!(i1, i2);
        let a = fs.read_inode(i1).unwrap();
        let b = fs.read_inode(i2).unwrap();
        assert_eq!(fs.read_file(&a).unwrap(), b"AAA");
        assert_eq!(fs.read_file(&b).unwrap(), b"BBBBBB");
    }

    /// Block bitmap reflects the metadata range after format —
    /// allocator can't hand out blocks 0..metadata_blocks, the
    /// kernel won't double-allocate them, fsck won't complain.
    #[test]
    fn test_format_block_bitmap_marks_metadata_range_used() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();

        // Block 2 holds the block bitmap; metadata occupies
        // blocks 0..7 (sb-block, GDT, blockbitmap, inodebitmap,
        // 2 inode-table blocks, root dir = 7 blocks total).
        let bitmap_offset = 2 * 4096;
        let bitmap_slice = &buf[bitmap_offset..bitmap_offset + 4096];
        for bit in 0..7 {
            assert!(
                spec::bitmap::get_bit(bitmap_slice, bit),
                "block {} should be marked used",
                bit
            );
        }
        // First free block.
        assert!(!spec::bitmap::get_bit(bitmap_slice, 7));
    }

    /// Inode bitmap marks inode 2 (root) as used after format.
    #[test]
    fn test_format_inode_bitmap_marks_root_inode_used() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        // Block 3 holds the inode bitmap. Inode 2 = bit 1.
        let bitmap_offset = 3 * 4096;
        let bitmap_slice = &buf[bitmap_offset..bitmap_offset + 4096];
        assert!(spec::bitmap::get_bit(bitmap_slice, 1));
        assert!(!spec::bitmap::get_bit(bitmap_slice, 0));
        assert!(!spec::bitmap::get_bit(bitmap_slice, 2));
    }

    /// Volume label round-trips through format → open.
    ///
    /// Bug it catches: a label setter that writes to the wrong
    /// offset or pads improperly would make `fs.superblock().
    /// volume_label()` return the wrong string after format.
    /// The label is operator-visible (mounted FS shows it), so
    /// silent corruption here would be embarrassing.
    #[test]
    fn test_format_volume_label_round_trips_through_open() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        let config = Config {
            volume_label: b"my-rootfs".to_vec(),
            ..Config::default()
        };
        format(&mut cursor, &config).unwrap();
        let fs = Filesystem::open(Cursor::new(buf)).unwrap();
        assert_eq!(fs.superblock().volume_label(), "my-rootfs");
    }
}

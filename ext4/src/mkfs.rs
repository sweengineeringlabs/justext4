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
//! **Kernel interop**: closed in both directions.
//!
//! - We open + walk real `mke2fs`-produced images
//!   (`tests/real_mkfs_roundtrip.rs`).
//! - `e2fsck -nf` accepts our output as a clean filesystem
//!   (`tests/e2fsck_acceptance.rs`, gated `#[ignore]`).
//! - The Linux kernel mounts our output as a real ext4
//!   filesystem (manual verification: `mount -o loop ...`).
//!
//! Getting there required emitting the fields the kernel +
//! e2fsck inspect that the v0 decoder doesn't itself need:
//! `s_state`, `s_errors`, `s_creator_os`, `s_first_ino`,
//! `s_max_mnt_count`, `s_log_cluster_size` (= log_block_size
//! when bigalloc is off), `s_clusters_per_group` (=
//! blocks_per_group), a non-zero UUID, hash seed, and the
//! `INCOMPAT_FILETYPE`, `INCOMPAT_EXTENTS`,
//! `RO_COMPAT_SPARSE_SUPER` feature bits. Plus bitmap
//! correctness: reserved-inode range marked used, padding past
//! the in-use range marked used, free counts matching the
//! bitmaps.
//!
//! What's enough for v0: produce something
//! [`crate::Filesystem::open`] can consume,
//! [`crate::Filesystem::read_inode(ROOT_INODE)`] can find as a
//! directory, and [`crate::Filesystem::read_dir`] can walk into
//! `[".", ".."]`. That's the round-trip demo this module is
//! designed to deliver.

use std::io::{Seek, SeekFrom, Write};

use spec::{
    bitmap, DirEntry, Extent, ExtentHeader, GroupDescriptor, Inode, Superblock,
    EXT4_ERRORS_CONTINUE, EXT4_HASH_HALF_MD4, EXT4_OS_LINUX, EXT4_VALID_FS,
    FEATURE_INCOMPAT_EXTENTS, FEATURE_INCOMPAT_FILETYPE, FEATURE_RO_COMPAT_SPARSE_SUPER,
    INODE_FLAG_EXTENTS, I_BLOCK_LEN, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE,
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
    // Free inodes = total - 10. The 10 reserved inodes (1..=10)
    // are marked used in the bitmap because e2fsck demands it,
    // even though only inode 2 (root) actually has content.
    // Free count must match what the bitmap says or e2fsck flags
    // a "Free inodes count wrong" inconsistency.
    let free_inodes = INODES_PER_GROUP - 10;

    // Pinned-deterministic timestamp + UUID + hash seed. mkfs.ext4
    // pulls these from the clock + RNG; we pin them so format()
    // output is byte-stable across runs (the same Config produces
    // the same image bytes). Reproducibility is a project guarantee.
    const PINNED_TIME: u32 = 0x6500_0000; // 2023-09-13 in posix epoch
    const PINNED_UUID: [u8; 16] = [
        0x6A, 0x75, 0x73, 0x74, 0x65, 0x78, 0x74, 0x34, 0x76, 0x30, 0xDE, 0xAD, 0xBE, 0xEF, 0xCA,
        0xFE,
    ];
    const PINNED_HASH_SEED: [u8; 16] = [
        0xEC, 0xCA, 0xFE, 0xBA, 0xBE, 0xDE, 0xAD, 0xBE, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC,
        0xDE,
    ];

    // Kernel convention: blocks_per_group = block_size * 8 (one
    // group's worth of blocks fits in a single block-bitmap
    // block). The total `size_blocks` decides how many groups
    // we need (typically 1 for tiny images).
    let blocks_per_group = config.block_size * 8;

    let sb = Superblock {
        block_size: config.block_size,
        inodes_count: INODES_PER_GROUP,
        blocks_count: total_blocks,
        free_blocks_count: free_blocks,
        free_inodes_count: free_inodes,
        inode_size: INODE_SIZE,
        rev_level: 1,
        feature_compat: 0,
        feature_incompat: FEATURE_INCOMPAT_FILETYPE | FEATURE_INCOMPAT_EXTENTS,
        feature_ro_compat: FEATURE_RO_COMPAT_SPARSE_SUPER,
        uuid: PINNED_UUID,
        volume_name,
        desc_size: 0, // ignored when 64BIT is clear
        first_data_block: 0,
        blocks_per_group,
        inodes_per_group: INODES_PER_GROUP,
        // Fields the kernel + e2fsck inspect during open. Setting
        // these to mke2fs-default values is what gets `e2fsck -nf`
        // to accept our output instead of rejecting it as a
        // corrupt superblock.
        mtime: 0,
        wtime: PINNED_TIME,
        mount_count: 0,
        max_mount_count: 0xFFFF, // -1 as i16: disable mount-count fsck
        state: EXT4_VALID_FS,
        errors: EXT4_ERRORS_CONTINUE,
        minor_rev_level: 0,
        last_check: PINNED_TIME,
        check_interval: 0,
        creator_os: EXT4_OS_LINUX,
        first_ino: 11, // standard reserved-inode count
        hash_seed: PINNED_HASH_SEED,
        def_hash_version: EXT4_HASH_HALF_MD4,
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

    // Block bitmap at block 2. Two regions get set:
    //  1. blocks 0..metadata_blocks — actually allocated (sb,
    //     GDT, both bitmaps, inode table, root dir).
    //  2. bits past the FS end, up through the bitmap-block
    //     boundary — the kernel calls this "padding"; bits
    //     marked used so allocators never hand out non-existent
    //     blocks. e2fsck warns when it's missing.
    let mut block_bitmap_buf = vec![0u8; config.block_size as usize];
    for i in 0..layout.metadata_blocks {
        bitmap::set_bit(&mut block_bitmap_buf, i as usize);
    }
    let bitmap_capacity_bits = (config.block_size as usize) * 8;
    for i in (total_blocks as usize)..bitmap_capacity_bits {
        bitmap::set_bit(&mut block_bitmap_buf, i);
    }
    writer.seek(SeekFrom::Start(2 * layout.block_size))?;
    writer.write_all(&block_bitmap_buf)?;

    // Inode bitmap at block 3. Three regions get set:
    //  1. The 10 reserved inodes (1..=10). The kernel hard-
    //     reserves these on every ext-family FS regardless of
    //     whether it actually uses them; e2fsck demands they be
    //     marked used.
    //  2. Inode 2 (root) — already covered by the reserved range
    //     but conceptually our actual allocation.
    //  3. Padding past inodes_per_group, same as for the block
    //     bitmap: prevents allocators from handing out indices
    //     that have no inode-table slot.
    let mut inode_bitmap_buf = vec![0u8; config.block_size as usize];
    for i in 0..10 {
        bitmap::set_bit(&mut inode_bitmap_buf, i);
    }
    bitmap::set_bit(&mut inode_bitmap_buf, (ROOT_INODE_NUMBER - 1) as usize);
    for i in (INODES_PER_GROUP as usize)..bitmap_capacity_bits {
        bitmap::set_bit(&mut inode_bitmap_buf, i);
    }
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

    /// Inode bitmap marks the 10 reserved inodes (1..=10) as
    /// used after format, plus pads bits past the in-use range
    /// up to the bitmap-block boundary.
    ///
    /// Bug it catches: e2fsck demands the conventional reserved-
    /// inode range be marked used regardless of whether the FS
    /// actually allocated content there. A formatter that only
    /// marks the inodes it really uses (just root, in our v0)
    /// produces e2fsck-rejected output.
    #[test]
    fn test_format_inode_bitmap_marks_reserved_range_used_and_pads_tail() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let bitmap_offset = 3 * 4096;
        let bitmap_slice = &buf[bitmap_offset..bitmap_offset + 4096];
        // Bits 0..10 (inodes 1..10): reserved range, must be set.
        for bit in 0..10 {
            assert!(
                spec::bitmap::get_bit(bitmap_slice, bit),
                "reserved inode {} should be marked used",
                bit + 1
            );
        }
        // Bit 32 onward (past INODES_PER_GROUP): padding, must
        // be set so allocators don't hand out non-existent
        // indices.
        assert!(spec::bitmap::get_bit(bitmap_slice, 32));
        // Bits 10..32 represent unallocated user inodes — must
        // be clear (free).
        for bit in 10..32 {
            assert!(
                !spec::bitmap::get_bit(bitmap_slice, bit),
                "user inode {} should be free",
                bit + 1
            );
        }
    }

    /// `mkdir` creates a new subdirectory whose `read_dir`
    /// returns the canonical `[".", ".."]` and whose `..` points
    /// back at the parent.
    ///
    /// Bug it catches: any field-offset error in mkdir's inode
    /// build (links_count, mode, extents flag, the embedded
    /// extent header) surfaces as either an open-failure or a
    /// read_dir that returns garbage. The bidirectional `..`
    /// check pins parent-link correctness — a parser writing
    /// `..` with the wrong inode number would fail the assertion.
    #[test]
    fn test_format_then_mkdir_creates_walkable_subdirectory() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        let sub_inode_num = fs.mkdir("/sub").unwrap();

        let sub = fs.read_inode(sub_inode_num).unwrap();
        assert!(sub.is_directory());
        assert_eq!(sub.links_count, 2);

        let entries = fs.read_dir(&sub).unwrap();
        let names: Vec<&[u8]> = entries
            .iter()
            .filter(|e| !e.is_unused())
            .map(|e| e.name.as_slice())
            .collect();
        assert!(names.iter().any(|n| *n == b"."));
        assert!(names.iter().any(|n| *n == b".."));
        let dotdot = entries.iter().find(|e| e.name == b"..").unwrap();
        assert_eq!(dotdot.inode, ROOT_INODE, "/sub/.. should point at root");
    }

    /// `mkdir` bumps the parent inode's links_count.
    ///
    /// Bug it catches: forgetting to update the parent makes
    /// e2fsck flag a "links_count wrong" inconsistency in pass
    /// 4. The kernel still mounts and reads the FS, but a
    /// subsequent rmdir on the new subdir would mis-account
    /// the parent's count.
    #[test]
    fn test_mkdir_increments_parent_links_count() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        let root_before = fs.read_inode(ROOT_INODE).unwrap();
        assert_eq!(root_before.links_count, 2);
        fs.mkdir("/sub").unwrap();
        let root_after = fs.read_inode(ROOT_INODE).unwrap();
        assert_eq!(
            root_after.links_count, 3,
            "root's links_count should bump by 1 after mkdir adds a child"
        );
    }

    /// File can be created inside a freshly-mkdir'd subdir, and
    /// resolved via `open_path("/sub/file.txt")`.
    ///
    /// Bug it catches: integration test for the full nested
    /// path. A mkdir that produces a directory whose data block
    /// can't be walked further (extent pointing at the wrong
    /// block, dir entries written at the wrong offset) would
    /// fail open_path on any nested target.
    #[test]
    fn test_mkdir_then_create_file_inside_resolves_via_nested_path() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        fs.mkdir("/etc").unwrap();
        let file_inode = fs.create_file("/etc/hostname", b"justext4-test").unwrap();
        assert_eq!(fs.open_path("/etc/hostname").unwrap(), file_inode);

        let inode = fs.read_inode(file_inode).unwrap();
        assert_eq!(fs.read_file(&inode).unwrap(), b"justext4-test");
    }

    /// `mkdir` rejects a name that already exists with
    /// AlreadyExists, regardless of whether the existing entry
    /// is a file or another dir.
    #[test]
    fn test_mkdir_collision_returns_already_exists() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        fs.create_file("/foo", b"existing file").unwrap();
        let err = fs.mkdir("/foo").unwrap_err();
        assert!(matches!(err, Ext4Error::AlreadyExists { .. }));
    }

    /// `unlink` removes a regular file: lookup misses after,
    /// space frees, file is gone from parent listing.
    ///
    /// Bug it catches: a remove that updates the bitmap but
    /// leaves the dir entry in place would have lookup still
    /// succeed (returning a tombstoned inode); a remove that
    /// updates the dir entry but not the bitmap would leak
    /// the blocks and inode forever. The lookup-after-unlink
    /// assertion is the user-facing contract; the free-count
    /// delta proves the bookkeeping.
    #[test]
    fn test_unlink_removes_file_and_returns_space() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        let free_inodes_before = fs.superblock().free_inodes_count;
        let free_blocks_before = fs.superblock().free_blocks_count;

        fs.create_file("/scratch.txt", b"about to be deleted")
            .unwrap();
        fs.unlink("/scratch.txt").unwrap();

        // Lookup must miss.
        let root = fs.read_inode(ROOT_INODE).unwrap();
        let err = fs.lookup(&root, b"scratch.txt").unwrap_err();
        assert!(matches!(err, Ext4Error::NotFound { .. }));

        // Free counts back to where they were before the create.
        assert_eq!(fs.superblock().free_inodes_count, free_inodes_before);
        assert_eq!(fs.superblock().free_blocks_count, free_blocks_before);
    }

    /// `unlink` rejects a directory inode with IsADirectory.
    ///
    /// Bug it catches: silently unlinking a directory orphans
    /// every entry inside it (inodes still allocated, blocks
    /// still owned, but no path to reach them). The kernel
    /// requires `rmdir` for directories — we mirror that.
    #[test]
    fn test_unlink_directory_returns_is_a_directory() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        fs.mkdir("/etc").unwrap();
        let err = fs.unlink("/etc").unwrap_err();
        assert!(matches!(err, Ext4Error::IsADirectory { .. }));
    }

    /// `unlink` of a missing entry returns NotFound.
    #[test]
    fn test_unlink_missing_file_returns_not_found() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        let err = fs.unlink("/never-existed").unwrap_err();
        assert!(matches!(err, Ext4Error::NotFound { .. }));
    }

    /// After `unlink`, the freed inode + blocks become available
    /// for a subsequent `create_file` of the same name.
    ///
    /// Bug it catches: a free that doesn't actually clear the
    /// bitmap bits would make the next allocation either fail
    /// (NoSpace despite plenty of room) or silently double-
    /// allocate (allocator hands out the same blocks already
    /// in use). The recreate-with-different-content test
    /// distinguishes: if the bitmap clears worked, the new
    /// file's content is what we just wrote; if not, content
    /// would be whatever stale bytes remained.
    #[test]
    fn test_unlink_then_create_same_name_with_different_content() {
        let mut buf: Vec<u8> = Vec::new();
        let mut cursor = Cursor::new(&mut buf);
        format(&mut cursor, &Config::default()).unwrap();
        let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();

        fs.create_file("/log.txt", b"first version").unwrap();
        fs.unlink("/log.txt").unwrap();
        let new_inode = fs.create_file("/log.txt", b"second version").unwrap();

        let inode = fs.read_inode(new_inode).unwrap();
        let bytes = fs.read_file(&inode).unwrap();
        assert_eq!(bytes, b"second version");
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

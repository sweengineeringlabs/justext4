//! Block group descriptor — one entry per block group, parked in
//! the GDT (Group Descriptor Table) immediately after the
//! superblock.
//!
//! Layout depends on `INCOMPAT_64BIT`:
//!
//! - non-64BIT: 32-byte entry, all addresses are u32 (block index).
//! - 64BIT: 64-byte entry, addresses split into hi+lo words and
//!   free-counts split into hi+lo u16s.
//!
//! The decoder consumes the entry size from
//! [`Superblock::group_descriptor_size`] and reads only that many
//! bytes; bytes past the entry are caller-owned.

use crate::superblock::Superblock;

// 32-byte (lo) field offsets — common to both layouts.
const OFF_BLOCK_BITMAP_LO: usize = 0x00;
const OFF_INODE_BITMAP_LO: usize = 0x04;
const OFF_INODE_TABLE_LO: usize = 0x08;
const OFF_FREE_BLOCKS_COUNT_LO: usize = 0x0C;
const OFF_FREE_INODES_COUNT_LO: usize = 0x0E;
const OFF_USED_DIRS_COUNT_LO: usize = 0x10;
const OFF_FLAGS: usize = 0x12;
const OFF_CHECKSUM: usize = 0x1E;

// 64-byte hi-word offsets — only valid when entry size >= 64.
const OFF_BLOCK_BITMAP_HI: usize = 0x20;
const OFF_INODE_BITMAP_HI: usize = 0x24;
const OFF_INODE_TABLE_HI: usize = 0x28;
const OFF_FREE_BLOCKS_COUNT_HI: usize = 0x2C;
const OFF_FREE_INODES_COUNT_HI: usize = 0x2E;
const OFF_USED_DIRS_COUNT_HI: usize = 0x30;

/// Group descriptor flag: inode table not yet zeroed. The kernel
/// sets this on freshly-allocated groups; readers must skip the
/// inode table for groups bearing it.
pub const BG_FLAG_INODE_UNINIT: u16 = 0x0001;

/// Group descriptor flag: block bitmap not yet initialised. Readers
/// treating an uninit group as fully-free is correct; treating it
/// as fully-used would cause a working filesystem to look full.
pub const BG_FLAG_BLOCK_UNINIT: u16 = 0x0002;

/// Group descriptor flag: inode table has been zeroed (clean state
/// after a `BG_FLAG_INODE_UNINIT` lazy-init pass).
pub const BG_FLAG_INODE_ZEROED: u16 = 0x0004;

/// Decoded block group descriptor. v0 fields are the load-bearing
/// ones for read-path traversal: where the bitmaps and inode table
/// live, how much is free, the lazy-init flags, and the kernel-
/// computed checksum.
///
/// Skipped for v0: `bg_exclude_bitmap_*` (snapshot feature),
/// `bg_block_bitmap_csum_*` / `bg_inode_bitmap_csum_*`
/// (`METADATA_CSUM` feature), and `bg_itable_unused_*` (scan-
/// optimisation hint). Add them when a caller needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDescriptor {
    /// Block address of the block bitmap. Combines hi/lo on 64BIT
    /// images.
    pub block_bitmap: u64,

    /// Block address of the inode bitmap. Combines hi/lo on 64BIT.
    pub inode_bitmap: u64,

    /// First block of the inode table for this group. Combines
    /// hi/lo on 64BIT.
    pub inode_table: u64,

    /// Free block count for this group. Combines u16 hi+lo into a
    /// u32 on 64BIT (groups can have more than 65k blocks free).
    pub free_blocks_count: u32,

    /// Free inode count for this group. Same hi+lo combining.
    pub free_inodes_count: u32,

    /// Directory count in this group — used by the kernel's
    /// "Orlov" allocator to bias new directories toward sparse
    /// groups.
    pub used_dirs_count: u32,

    /// `bg_flags` — see [`BG_FLAG_INODE_UNINIT`],
    /// [`BG_FLAG_BLOCK_UNINIT`], [`BG_FLAG_INODE_ZEROED`].
    pub flags: u16,

    /// `bg_checksum` — CRC16 (or CRC32C when `METADATA_CSUM` is
    /// enabled) over the descriptor entry. Stored verbatim; v0
    /// does not validate it. Validation lands with the metadata-
    /// csum support pass.
    pub checksum: u16,
}

impl GroupDescriptor {
    /// Decode a group descriptor entry. The caller passes the
    /// superblock so the decoder can pick the right entry size and
    /// hi/lo combining rule. The buffer must contain at least
    /// `sb.group_descriptor_size()` bytes.
    pub fn decode(buf: &[u8], sb: &Superblock) -> Result<Self, GroupDescriptorDecodeError> {
        let entry_size = sb.group_descriptor_size() as usize;

        if entry_size < 32 {
            return Err(GroupDescriptorDecodeError::EntrySizeTooSmall {
                size: entry_size as u16,
            });
        }

        if buf.len() < entry_size {
            return Err(GroupDescriptorDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: entry_size,
            });
        }

        let block_bitmap_lo = read_u32(buf, OFF_BLOCK_BITMAP_LO);
        let inode_bitmap_lo = read_u32(buf, OFF_INODE_BITMAP_LO);
        let inode_table_lo = read_u32(buf, OFF_INODE_TABLE_LO);
        let free_blocks_lo = read_u16(buf, OFF_FREE_BLOCKS_COUNT_LO);
        let free_inodes_lo = read_u16(buf, OFF_FREE_INODES_COUNT_LO);
        let used_dirs_lo = read_u16(buf, OFF_USED_DIRS_COUNT_LO);
        let flags = read_u16(buf, OFF_FLAGS);
        let checksum = read_u16(buf, OFF_CHECKSUM);

        let (
            block_bitmap_hi,
            inode_bitmap_hi,
            inode_table_hi,
            free_blocks_hi,
            free_inodes_hi,
            used_dirs_hi,
        ) = if entry_size >= 64 {
            (
                read_u32(buf, OFF_BLOCK_BITMAP_HI),
                read_u32(buf, OFF_INODE_BITMAP_HI),
                read_u32(buf, OFF_INODE_TABLE_HI),
                read_u16(buf, OFF_FREE_BLOCKS_COUNT_HI),
                read_u16(buf, OFF_FREE_INODES_COUNT_HI),
                read_u16(buf, OFF_USED_DIRS_COUNT_HI),
            )
        } else {
            (0, 0, 0, 0, 0, 0)
        };

        Ok(GroupDescriptor {
            block_bitmap: combine_addr(block_bitmap_hi, block_bitmap_lo),
            inode_bitmap: combine_addr(inode_bitmap_hi, inode_bitmap_lo),
            inode_table: combine_addr(inode_table_hi, inode_table_lo),
            free_blocks_count: combine_count(free_blocks_hi, free_blocks_lo),
            free_inodes_count: combine_count(free_inodes_hi, free_inodes_lo),
            used_dirs_count: combine_count(used_dirs_hi, used_dirs_lo),
            flags,
            checksum,
        })
    }

    /// True iff the block bitmap for this group is uninitialised
    /// (lazy-init not yet completed). Bitmap reads on these groups
    /// should treat all blocks as free.
    pub fn is_block_uninit(&self) -> bool {
        self.flags & BG_FLAG_BLOCK_UNINIT != 0
    }

    /// True iff the inode bitmap is uninitialised.
    pub fn is_inode_uninit(&self) -> bool {
        self.flags & BG_FLAG_INODE_UNINIT != 0
    }
}

fn combine_addr(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | (lo as u64)
}

fn combine_count(hi: u16, lo: u16) -> u32 {
    ((hi as u32) << 16) | (lo as u32)
}

fn read_u16(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GroupDescriptorDecodeError {
    #[error("group descriptor entry size {size} is below the kernel minimum of 32")]
    EntrySizeTooSmall { size: u16 },

    #[error(
        "input too small to contain a group descriptor: have {actual} bytes, expected {expected}"
    )]
    InputTooSmall { actual: usize, expected: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superblock::{Superblock, EXT4_MAGIC, FEATURE_INCOMPAT_64BIT, SUPERBLOCK_SIZE};

    /// Build a superblock buffer with caller-controlled feature bits
    /// and `s_desc_size`. Returns the decoded `Superblock` so tests
    /// can pass it to [`GroupDescriptor::decode`] without re-parsing.
    fn make_sb(feature_incompat: u32, desc_size: u16) -> Superblock {
        // Field offsets are private to the superblock module, but
        // the public API lets us construct a buffer through it.
        let mut buf = vec![0u8; SUPERBLOCK_SIZE];
        // s_log_block_size = 2 (4 KiB)
        buf[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes());
        // s_magic = 0xEF53
        buf[0x38..0x3A].copy_from_slice(&EXT4_MAGIC.to_le_bytes());
        // s_rev_level = 1
        buf[0x4C..0x50].copy_from_slice(&1u32.to_le_bytes());
        // s_feature_incompat
        buf[0x60..0x64].copy_from_slice(&feature_incompat.to_le_bytes());
        // s_desc_size at 0xFE
        buf[0xFE..0x100].copy_from_slice(&desc_size.to_le_bytes());
        Superblock::decode(&buf).unwrap()
    }

    fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Truncated buffer must surface a typed error.
    ///
    /// Bug it catches: a parser that indexes into `buf` before
    /// length-checking would panic when the caller passes a partial
    /// read of the GDT (e.g. last group's entry partially read at
    /// disk-end). The typed error lets the caller route on it.
    #[test]
    fn test_decode_truncated_buffer_returns_input_too_small() {
        let sb = make_sb(0, 0);
        let buf = vec![0u8; 16]; // < 32
        let err = GroupDescriptor::decode(&buf, &sb).unwrap_err();
        assert_eq!(
            err,
            GroupDescriptorDecodeError::InputTooSmall {
                actual: 16,
                expected: 32,
            }
        );
    }

    /// 32-byte entry: `block_bitmap` is the lo-word value alone.
    ///
    /// Bug it catches: a 32-bit-image decoder that reads past
    /// offset 32 to fetch a hi-word would either OOB-read (if the
    /// caller passed exactly a 32-byte slice) or pick up bytes
    /// belonging to the *next* GDT entry, scrambling addresses
    /// across the table.
    #[test]
    fn test_decode_32_byte_entry_block_bitmap_is_lo_only() {
        let sb = make_sb(0, 0);
        let mut buf = vec![0u8; 32];
        write_u32(&mut buf, OFF_BLOCK_BITMAP_LO, 0x1234_5678);
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert_eq!(gd.block_bitmap, 0x1234_5678);
    }

    /// 32-byte decode does not read past entry boundary even when
    /// the buffer is bigger.
    ///
    /// Bug it catches: a parser that always reads 64 bytes (because
    /// it assumes 64-bit) on a 32-bit-feature image would pull bytes
    /// from beyond the entry — for in-memory test buffers, those
    /// bytes are caller-controlled and could happen to be plausible
    /// values that mask the bug. Forcing 0xFF in the trailing bytes
    /// surfaces it: a wrong decoder would report block_bitmap as
    /// e.g. 0xFFFF_FFFF_0000_0001 instead of 0x0000_0001.
    #[test]
    fn test_decode_32_byte_entry_ignores_trailing_bytes_in_oversize_buffer() {
        let sb = make_sb(0, 0);
        let mut buf = vec![0xFFu8; 64];
        // Zero the first 32 bytes (the actual entry), set lo to a
        // sentinel.
        for byte in buf.iter_mut().take(32) {
            *byte = 0;
        }
        write_u32(&mut buf, OFF_BLOCK_BITMAP_LO, 0x0000_0001);
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert_eq!(gd.block_bitmap, 0x0000_0001);
    }

    /// 64-byte entry: `block_bitmap` combines hi and lo into a u64.
    ///
    /// Bug it catches: a decoder that ignores the hi word on 64-bit
    /// images (the most common bug pattern when extending a 32-bit
    /// reader) silently truncates addresses to 32 bits, breaking
    /// images larger than 16 TiB.
    #[test]
    fn test_decode_64_byte_entry_block_bitmap_combines_hi_lo() {
        let sb = make_sb(FEATURE_INCOMPAT_64BIT, 64);
        let mut buf = vec![0u8; 64];
        write_u32(&mut buf, OFF_BLOCK_BITMAP_LO, 0x0000_0001);
        write_u32(&mut buf, OFF_BLOCK_BITMAP_HI, 0x0000_0002);
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert_eq!(gd.block_bitmap, 0x0000_0002_0000_0001);
    }

    /// 64-byte free counts combine u16 hi+lo into a u32.
    ///
    /// Bug it catches: a decoder that returns just the lo u16 caps
    /// reported free counts at 65535. On modern ext4 with very
    /// large block groups (32k blocks per group is the default;
    /// the maximum is far higher), this loses the high bits of the
    /// count and grossly under-reports free space, causing the
    /// allocator to fail spuriously.
    #[test]
    fn test_decode_64_byte_free_blocks_combines_hi_lo() {
        let sb = make_sb(FEATURE_INCOMPAT_64BIT, 64);
        let mut buf = vec![0u8; 64];
        write_u16(&mut buf, OFF_FREE_BLOCKS_COUNT_LO, 0x1234);
        write_u16(&mut buf, OFF_FREE_BLOCKS_COUNT_HI, 0x0005);
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert_eq!(gd.free_blocks_count, 0x0005_1234);
    }

    /// `BG_FLAG_BLOCK_UNINIT` is detectable via [`is_block_uninit`].
    ///
    /// Bug it catches: a decoder that reads `bg_flags` as a u8
    /// instead of u16 would lose the high byte. While today's flags
    /// all live in the low byte, future flag bits will not — and
    /// readers that treat uninit groups as zero-free space cause
    /// allocators to think a fresh disk is full.
    #[test]
    fn test_block_uninit_flag_detected_on_freshly_initialised_group() {
        let sb = make_sb(0, 0);
        let mut buf = vec![0u8; 32];
        write_u16(&mut buf, OFF_FLAGS, BG_FLAG_BLOCK_UNINIT);
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert!(gd.is_block_uninit());
        assert!(!gd.is_inode_uninit());
    }

    /// Empty (all-zero) 32-byte entry decodes to all-zero fields.
    ///
    /// Bug it catches: a decoder that mistakenly applies
    /// hi/lo combining on a 32-bit image (reading past entry end)
    /// would pull non-zero bytes from beyond the buffer if the
    /// caller didn't zero them. The test passes only when the
    /// decoder honours `entry_size = 32` and skips the hi reads
    /// entirely.
    #[test]
    fn test_decode_zeroed_32_byte_entry_produces_all_zero_descriptor() {
        let sb = make_sb(0, 0);
        let buf = vec![0u8; 32];
        let gd = GroupDescriptor::decode(&buf, &sb).unwrap();
        assert_eq!(gd.block_bitmap, 0);
        assert_eq!(gd.inode_bitmap, 0);
        assert_eq!(gd.inode_table, 0);
        assert_eq!(gd.free_blocks_count, 0);
        assert_eq!(gd.free_inodes_count, 0);
        assert_eq!(gd.used_dirs_count, 0);
        assert_eq!(gd.flags, 0);
        assert_eq!(gd.checksum, 0);
    }
}

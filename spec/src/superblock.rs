//! Ext4 superblock — decode of the 1024-byte struct at byte offset
//! 1024 within the partition.
//!
//! v0 scope: decode the load-bearing fields a consumer needs to make
//! sense of an image (block size, total / free counts, UUID, volume
//! label, feature flags, inode size). Encode + the long-tail fields
//! land alongside the write path.

/// Byte offset of the superblock within the partition. Independent
/// of block size; ext4 always places the primary superblock at
/// byte 1024.
pub const SUPERBLOCK_OFFSET: u64 = 1024;

/// Size of the superblock struct on disk. The kernel layout is
/// 1024 bytes; later fields beyond rev 1 are zero-padded.
pub const SUPERBLOCK_SIZE: usize = 1024;

/// ext2 / ext3 / ext4 magic at offset 0x38. ext4 reuses the same
/// magic; the discriminator is in `s_feature_incompat`.
pub const EXT4_MAGIC: u16 = 0xEF53;

// Field offsets within the superblock buffer (not the partition).
// Sourced from kernel `fs/ext4/ext4.h` `struct ext4_super_block`.
const OFF_INODES_COUNT: usize = 0x00;
const OFF_BLOCKS_COUNT_LO: usize = 0x04;
const OFF_FREE_BLOCKS_COUNT_LO: usize = 0x0C;
const OFF_FREE_INODES_COUNT: usize = 0x10;
const OFF_LOG_BLOCK_SIZE: usize = 0x18;
const OFF_REV_LEVEL: usize = 0x4C;
const OFF_INODE_SIZE: usize = 0x58;
const OFF_MAGIC: usize = 0x38;
const OFF_FEATURE_COMPAT: usize = 0x5C;
const OFF_FEATURE_INCOMPAT: usize = 0x60;
const OFF_FEATURE_RO_COMPAT: usize = 0x64;
const OFF_UUID: usize = 0x68;
const OFF_VOLUME_NAME: usize = 0x78;
const OFF_DESC_SIZE: usize = 0xFE;
const OFF_BLOCKS_COUNT_HI: usize = 0x150;
const OFF_FREE_BLOCKS_COUNT_HI: usize = 0x154;

/// Revision level at which `s_inode_size` and other dynamic fields
/// became valid. Rev 0 has a fixed 128-byte inode and no dynamic
/// fields.
const REV_DYNAMIC: u32 = 1;

/// Default inode size for rev 0 (and for rev 1 images where the
/// field reads as zero, which the kernel treats as "use the rev-0
/// default").
const DEFAULT_INODE_SIZE: u16 = 128;

/// `INCOMPAT_64BIT` — when set, total block count uses
/// `s_blocks_count_hi` as the high 32 bits, and the group descriptor
/// table uses 64-byte (rather than 32-byte) entries with hi/lo split
/// addresses. Without this flag both hi words are reserved and must
/// be ignored on read.
pub const FEATURE_INCOMPAT_64BIT: u32 = 0x80;

/// Group descriptor size when `INCOMPAT_64BIT` is *not* set. The
/// kernel hard-codes this as `EXT4_MIN_DESC_SIZE`. Older ext2/3
/// images and 32-bit ext4 images use it.
pub const GDT_ENTRY_SIZE_32: u16 = 32;

/// Decoded ext4 superblock — only the v0 fields. Fields the higher
/// layers do not yet need (mount counts, last-write timestamps,
/// journal inode, MMP block) are intentionally omitted; adding them
/// is mechanical when a caller needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superblock {
    /// Bytes per block (1 KiB / 2 KiB / 4 KiB / 64 KiB).
    pub block_size: u32,

    /// Total inodes in the filesystem.
    pub inodes_count: u32,

    /// Total blocks. Combines `s_blocks_count_lo` with
    /// `s_blocks_count_hi` when the 64BIT feature is set; otherwise
    /// the hi word is ignored.
    pub blocks_count: u64,

    /// Free blocks. Same hi/lo combining rule as `blocks_count`.
    pub free_blocks_count: u64,

    /// Free inodes.
    pub free_inodes_count: u32,

    /// Inode size on disk. 128 for rev 0; rev 1+ images report the
    /// actual size in `s_inode_size` (typically 256).
    pub inode_size: u16,

    /// 0 = original (rev 0); 1 = dynamic (rev 1, ext2/3/4 with
    /// dynamic fields). Higher revisions are not currently observed
    /// in the wild but are decoded as-is.
    pub rev_level: u32,

    /// `s_feature_compat` — features the kernel may ignore safely
    /// when not understood.
    pub feature_compat: u32,

    /// `s_feature_incompat` — kernel must understand all set bits
    /// to mount.
    pub feature_incompat: u32,

    /// `s_feature_ro_compat` — kernel may mount read-only when
    /// unknown bits are set.
    pub feature_ro_compat: u32,

    /// Volume UUID. Bytes 0..16 verbatim from disk.
    pub uuid: [u8; 16],

    /// Volume label, nul-padded. Use [`Superblock::volume_label`]
    /// for the human-readable form with the nul padding stripped.
    pub volume_name: [u8; 16],

    /// Raw `s_desc_size` field (offset 0xFE). On non-64BIT images
    /// this is reserved and ignored — use
    /// [`Superblock::group_descriptor_size`] for the value the
    /// kernel actually applies.
    pub desc_size: u16,
}

impl Superblock {
    /// Decode a superblock from a 1024-byte (or larger) buffer
    /// starting at the superblock boundary. The caller is
    /// responsible for seeking to [`SUPERBLOCK_OFFSET`] within the
    /// partition before reading the bytes; this function does not
    /// do IO.
    pub fn decode(buf: &[u8]) -> Result<Self, SuperblockDecodeError> {
        if buf.len() < SUPERBLOCK_SIZE {
            return Err(SuperblockDecodeError::InputTooSmall { actual: buf.len() });
        }

        let magic = read_u16(buf, OFF_MAGIC);
        if magic != EXT4_MAGIC {
            return Err(SuperblockDecodeError::BadMagic { found: magic });
        }

        let log_block_size = read_u32(buf, OFF_LOG_BLOCK_SIZE);
        let block_size = match log_block_size {
            // Kernel rejects anything outside this set. The values
            // are sparse on purpose: 1KiB/2KiB/4KiB/64KiB.
            0 | 1 | 2 | 6 => 1024u32 << log_block_size,
            other => {
                return Err(SuperblockDecodeError::InvalidBlockSize {
                    log_block_size: other,
                });
            }
        };

        let rev_level = read_u32(buf, OFF_REV_LEVEL);
        let inode_size = if rev_level >= REV_DYNAMIC {
            let v = read_u16(buf, OFF_INODE_SIZE);
            if v == 0 {
                DEFAULT_INODE_SIZE
            } else {
                v
            }
        } else {
            DEFAULT_INODE_SIZE
        };

        let feature_compat = read_u32(buf, OFF_FEATURE_COMPAT);
        let feature_incompat = read_u32(buf, OFF_FEATURE_INCOMPAT);
        let feature_ro_compat = read_u32(buf, OFF_FEATURE_RO_COMPAT);

        let is_64bit = (feature_incompat & FEATURE_INCOMPAT_64BIT) != 0;
        let blocks_count = combine_hi_lo(
            read_u32(buf, OFF_BLOCKS_COUNT_HI),
            read_u32(buf, OFF_BLOCKS_COUNT_LO),
            is_64bit,
        );
        let free_blocks_count = combine_hi_lo(
            read_u32(buf, OFF_FREE_BLOCKS_COUNT_HI),
            read_u32(buf, OFF_FREE_BLOCKS_COUNT_LO),
            is_64bit,
        );

        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&buf[OFF_UUID..OFF_UUID + 16]);
        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&buf[OFF_VOLUME_NAME..OFF_VOLUME_NAME + 16]);

        Ok(Superblock {
            block_size,
            inodes_count: read_u32(buf, OFF_INODES_COUNT),
            blocks_count,
            free_blocks_count,
            free_inodes_count: read_u32(buf, OFF_FREE_INODES_COUNT),
            inode_size,
            rev_level,
            feature_compat,
            feature_incompat,
            feature_ro_compat,
            uuid,
            volume_name,
            desc_size: read_u16(buf, OFF_DESC_SIZE),
        })
    }

    /// True iff `INCOMPAT_64BIT` is set. 64-bit images use 64-byte
    /// group descriptors with hi/lo split addresses and combine
    /// `s_blocks_count_hi` into the total block count.
    pub fn is_64bit(&self) -> bool {
        (self.feature_incompat & FEATURE_INCOMPAT_64BIT) != 0
    }

    /// Group descriptor entry size in bytes, applying the kernel's
    /// rule: 32 (`EXT4_MIN_DESC_SIZE`) on non-64BIT images, raw
    /// `s_desc_size` on 64BIT images.
    ///
    /// The raw `desc_size` field is only valid when 64BIT is set;
    /// on 32-bit images it's reserved and may contain stale bytes.
    /// Routing the read through this method keeps callers off the
    /// raw field.
    pub fn group_descriptor_size(&self) -> u16 {
        if self.is_64bit() {
            self.desc_size
        } else {
            GDT_ENTRY_SIZE_32
        }
    }

    /// Volume label as a `&str`, with trailing nul padding stripped.
    /// Returns `""` if the label is empty or contains non-UTF-8 bytes
    /// before the first nul.
    pub fn volume_label(&self) -> &str {
        let end = self
            .volume_name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.volume_name.len());
        std::str::from_utf8(&self.volume_name[..end]).unwrap_or("")
    }
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

fn combine_hi_lo(hi: u32, lo: u32, is_64bit: bool) -> u64 {
    if is_64bit {
        ((hi as u64) << 32) | (lo as u64)
    } else {
        lo as u64
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SuperblockDecodeError {
    #[error("input too small to contain a superblock: have {actual} bytes, need 1024")]
    InputTooSmall { actual: usize },

    #[error("bad ext4 magic: found 0x{found:04x}, expected 0xef53")]
    BadMagic { found: u16 },

    #[error("invalid s_log_block_size {log_block_size}: only 0, 1, 2, 6 are valid (1 KiB / 2 KiB / 4 KiB / 64 KiB blocks)")]
    InvalidBlockSize { log_block_size: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 1024-byte superblock buffer with caller-controlled
    /// magic, log_block_size, rev_level, and feature_incompat. All
    /// other fields are zero unless overwritten by the test before
    /// calling [`Superblock::decode`].
    fn buf_with(magic: u16, log_block_size: u32, rev_level: u32, feature_incompat: u32) -> Vec<u8> {
        let mut buf = vec![0u8; SUPERBLOCK_SIZE];
        write_u32(&mut buf, OFF_LOG_BLOCK_SIZE, log_block_size);
        buf[OFF_MAGIC..OFF_MAGIC + 2].copy_from_slice(&magic.to_le_bytes());
        write_u32(&mut buf, OFF_REV_LEVEL, rev_level);
        write_u32(&mut buf, OFF_FEATURE_INCOMPAT, feature_incompat);
        buf
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Truncated input must return InputTooSmall, not panic via OOB
    /// indexing.
    ///
    /// Bug it catches: a parser that indexes into the buffer before
    /// length-checking will panic on a truncated read (e.g. a partial
    /// disk read of 512 bytes), instead of surfacing a typed error
    /// the caller can route on.
    #[test]
    fn test_decode_input_smaller_than_superblock_returns_input_too_small() {
        let short = vec![0u8; 512];
        let err = Superblock::decode(&short).unwrap_err();
        assert_eq!(err, SuperblockDecodeError::InputTooSmall { actual: 512 });
    }

    /// Wrong magic must reject, not silently parse garbage.
    ///
    /// Bug it catches: a parser that skips magic validation will
    /// happily decode block sizes and counts from a non-ext4 buffer
    /// (e.g. a zeroed disk, an XFS partition, a random file), leading
    /// to nonsense reported back to the user as if it were a valid
    /// filesystem.
    #[test]
    fn test_decode_non_ext4_magic_returns_bad_magic() {
        let buf = buf_with(0xDEAD, 2, 1, 0);
        let err = Superblock::decode(&buf).unwrap_err();
        assert_eq!(err, SuperblockDecodeError::BadMagic { found: 0xDEAD });
    }

    /// `s_log_block_size = 30` would overflow `1024 << 30` if not
    /// validated; the decoder must reject it.
    ///
    /// Bug it catches: a parser that does `1024 << s_log_block_size`
    /// without bounds-checking either overflows u32 (UB-equivalent
    /// in release: silent wraparound to 0) or produces a block size
    /// the kernel would never have written. Either way, downstream
    /// code computing offsets from `block_size` produces garbage.
    #[test]
    fn test_decode_invalid_log_block_size_returns_invalid_block_size() {
        let buf = buf_with(EXT4_MAGIC, 30, 1, 0);
        let err = Superblock::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            SuperblockDecodeError::InvalidBlockSize { log_block_size: 30 }
        );
    }

    /// Standard 4 KiB block: `s_log_block_size = 2` → `block_size =
    /// 4096`.
    ///
    /// Bug it catches: a parser that returns the raw `s_log_block_size`
    /// field value (2) as the block size, instead of computing
    /// `1024 << s_log_block_size` (4096). Every downstream offset
    /// computation breaks silently because the values are similar in
    /// magnitude.
    #[test]
    fn test_decode_log_block_size_2_yields_4kib_block_size() {
        let buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.block_size, 4096);
    }

    /// 1 KiB block: `s_log_block_size = 0` → `block_size = 1024`.
    ///
    /// Bug it catches: a parser that special-cases shift = 0 to
    /// "default 4 KiB" instead of computing 1024 << 0. Smaller
    /// floppy-style images and recovery filesystems use 1 KiB.
    #[test]
    fn test_decode_log_block_size_0_yields_1kib_block_size() {
        let buf = buf_with(EXT4_MAGIC, 0, 1, 0);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.block_size, 1024);
    }

    /// With `INCOMPAT_64BIT` set, `blocks_count` must combine hi
    /// and lo words.
    ///
    /// Bug it catches: a parser that only reads
    /// `s_blocks_count_lo` will undercount filesystems larger than
    /// 16 TiB (the 32-bit limit at 4 KiB blocks), silently truncating
    /// the high half. The kernel allocates blocks above 2^32 in
    /// 64-bit images; misreading this number causes downstream
    /// allocator code to think the disk is empty when it's not.
    #[test]
    fn test_decode_blocks_count_combines_hi_when_64bit_feature_set() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, FEATURE_INCOMPAT_64BIT);
        write_u32(&mut buf, OFF_BLOCKS_COUNT_LO, 0x0000_0001);
        write_u32(&mut buf, OFF_BLOCKS_COUNT_HI, 0x0000_0002);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.blocks_count, 0x0000_0002_0000_0001);
    }

    /// Without `INCOMPAT_64BIT`, the hi word is reserved and must
    /// be ignored even if non-zero.
    ///
    /// Bug it catches: a parser that unconditionally combines hi and
    /// lo will inflate the block count on a 32-bit ext4 image where
    /// the hi field happens to contain stale or reserved bytes,
    /// producing absurd values (10s of TiB on a 100 MiB image) that
    /// the allocator would then try to honour.
    #[test]
    fn test_decode_blocks_count_ignores_hi_when_64bit_feature_clear() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        write_u32(&mut buf, OFF_BLOCKS_COUNT_LO, 0x0000_0042);
        write_u32(&mut buf, OFF_BLOCKS_COUNT_HI, 0xFFFF_FFFF);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.blocks_count, 0x0000_0042);
    }

    /// `s_inode_size` is only valid when `s_rev_level >= 1`. On
    /// rev 0 the field is reserved; the kernel uses 128.
    ///
    /// Bug it catches: a parser that reads `s_inode_size` on rev-0
    /// images would interpret reserved bytes (often zero, sometimes
    /// garbage) as the inode size, leading to wrong inode-table
    /// offset computations downstream.
    #[test]
    fn test_decode_rev_0_returns_default_128_byte_inode_size() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 0, 0);
        // Pollute the inode_size field; the decoder must ignore it
        // because rev_level == 0.
        buf[OFF_INODE_SIZE..OFF_INODE_SIZE + 2].copy_from_slice(&512u16.to_le_bytes());
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.inode_size, 128);
    }

    /// On rev 1+, a zero `s_inode_size` field means "use the rev-0
    /// default of 128", per the kernel.
    ///
    /// Bug it catches: a parser that returns the raw zero would
    /// later divide by inode_size (e.g. when stepping through the
    /// inode table) and panic on division-by-zero.
    #[test]
    fn test_decode_rev_1_with_zero_inode_size_field_returns_128() {
        let buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.inode_size, 128);
    }

    /// On rev 1, an explicit inode_size of 256 must round-trip.
    ///
    /// Bug it catches: a parser that always returns 128 on the
    /// belief that "ext4 always uses 128-byte inodes" — wrong since
    /// ~2008; modern mkfs.ext4 produces 256-byte inodes by default.
    #[test]
    fn test_decode_rev_1_with_inode_size_256_round_trips() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        buf[OFF_INODE_SIZE..OFF_INODE_SIZE + 2].copy_from_slice(&256u16.to_le_bytes());
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.inode_size, 256);
    }

    /// UUID bytes must be returned verbatim — no endian flip, no
    /// byte reorder.
    ///
    /// Bug it catches: a parser that decodes `s_uuid` as a sequence
    /// of integers (and so endian-flips them) would produce a
    /// different UUID than every other ext4 tool. The bytes are an
    /// opaque 128-bit identifier on disk, not structured numeric
    /// fields.
    #[test]
    fn test_decode_uuid_is_byte_for_byte_verbatim() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let uuid_bytes = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        buf[OFF_UUID..OFF_UUID + 16].copy_from_slice(&uuid_bytes);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.uuid, uuid_bytes);
    }

    /// Volume label trims trailing nul padding before the caller
    /// sees it.
    ///
    /// Bug it catches: a parser that hands back the raw 16-byte
    /// volume_name field would let the embedded nuls leak into UI
    /// strings ("rootfs\0\0\0\0\0\0\0\0\0\0"), breaking display and
    /// any downstream filename / path use.
    #[test]
    fn test_volume_label_strips_trailing_nul_padding() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let label = b"rootfs";
        buf[OFF_VOLUME_NAME..OFF_VOLUME_NAME + label.len()].copy_from_slice(label);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.volume_label(), "rootfs");
    }

    /// Empty volume label (all-nul field) returns the empty string.
    ///
    /// Bug it catches: a parser that searches for the first nul and
    /// then panics on `volume_name[0..0]` — or worse, returns the
    /// full nul-byte buffer as a label — would crash or misrender
    /// images with no label set, which is the default for many
    /// builders.
    #[test]
    fn test_volume_label_empty_field_returns_empty_string() {
        let buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.volume_label(), "");
    }

    /// `is_64bit()` is false when `INCOMPAT_64BIT` is clear.
    ///
    /// Bug it catches: callers that branch on `is_64bit()` (notably
    /// the GDT decoder choosing 32-byte vs 64-byte entries) would
    /// otherwise need to test the raw bit themselves and could pick
    /// the wrong mask, especially since the bit value (0x80) is
    /// easily confused with other feature bits.
    #[test]
    fn test_is_64bit_false_when_feature_clear() {
        let buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        let sb = Superblock::decode(&buf).unwrap();
        assert!(!sb.is_64bit());
    }

    /// `is_64bit()` is true when `INCOMPAT_64BIT` is set.
    #[test]
    fn test_is_64bit_true_when_feature_set() {
        let buf = buf_with(EXT4_MAGIC, 2, 1, FEATURE_INCOMPAT_64BIT);
        let sb = Superblock::decode(&buf).unwrap();
        assert!(sb.is_64bit());
    }

    /// `group_descriptor_size()` returns 32 on non-64BIT images
    /// even when the raw `s_desc_size` field is non-zero.
    ///
    /// Bug it catches: a parser that returns the raw `desc_size`
    /// unconditionally would compute wrong-sized GDT entries on
    /// 32-bit images where the field is reserved. With this rule,
    /// the GDT decoder gets 32 (the kernel's `EXT4_MIN_DESC_SIZE`)
    /// regardless of stale bytes in the reserved field.
    #[test]
    fn test_group_descriptor_size_returns_32_on_non_64bit_regardless_of_field() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, 0);
        // Pollute the reserved s_desc_size field. Decoder must ignore
        // it because the 64BIT feature bit is clear.
        buf[OFF_DESC_SIZE..OFF_DESC_SIZE + 2].copy_from_slice(&64u16.to_le_bytes());
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.group_descriptor_size(), 32);
    }

    /// `group_descriptor_size()` returns the raw `s_desc_size` on
    /// 64BIT images.
    ///
    /// Bug it catches: a parser that hard-codes 32 on the assumption
    /// "every ext4 image uses 32-byte descriptors" misreads modern
    /// 64-bit images where the GDT entries are 64 bytes. Every
    /// hi-word field (block_bitmap_hi, inode_table_hi, etc.) would
    /// be silently dropped.
    #[test]
    fn test_group_descriptor_size_returns_raw_field_on_64bit() {
        let mut buf = buf_with(EXT4_MAGIC, 2, 1, FEATURE_INCOMPAT_64BIT);
        buf[OFF_DESC_SIZE..OFF_DESC_SIZE + 2].copy_from_slice(&64u16.to_le_bytes());
        let sb = Superblock::decode(&buf).unwrap();
        assert_eq!(sb.group_descriptor_size(), 64);
    }
}

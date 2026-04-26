//! Ext4 inode — fixed-size record describing a single file or
//! directory.
//!
//! Sizing depends on `Superblock::inode_size`:
//!
//! - rev 0 images: always 128 bytes; only the ext2-compatible
//!   fields are present.
//! - rev 1+ images: typically 256 bytes (`mkfs.ext4` default
//!   since ~2008); bytes 128+ hold ext4 extras (extra-precision
//!   timestamps, project ID, checksum hi).
//!
//! v0 scope: decode the load-bearing first-128-byte fields needed
//! to traverse a read-mode filesystem — mode, hi+lo size, hi+lo uid
//! and gid (Linux OSD2 layout), links count, flags, the raw 60-byte
//! `i_block` array (extent header or indirect pointers, decoded by
//! a higher layer), and the standard timestamps.
//!
//! Skipped for v0 (mechanical to add): extra-precision nanosecond
//! timestamps, crtime, projid, file ACL, inode checksum.

use crate::superblock::Superblock;

const OFF_MODE: usize = 0x00;
const OFF_UID_LO: usize = 0x02;
const OFF_SIZE_LO: usize = 0x04;
const OFF_ATIME: usize = 0x08;
const OFF_CTIME: usize = 0x0C;
const OFF_MTIME: usize = 0x10;
const OFF_DTIME: usize = 0x14;
const OFF_GID_LO: usize = 0x18;
const OFF_LINKS_COUNT: usize = 0x1A;
const OFF_BLOCKS_LO: usize = 0x1C;
const OFF_FLAGS: usize = 0x20;
const OFF_BLOCK: usize = 0x28;
const OFF_GENERATION: usize = 0x64;
const OFF_FILE_ACL_LO: usize = 0x68;
const OFF_SIZE_HI: usize = 0x6C;
// Linux OSD2 layout (struct linux2 in the kernel union).
const OFF_BLOCKS_HI: usize = 0x74;
const OFF_FILE_ACL_HI: usize = 0x76;
const OFF_UID_HI: usize = 0x78;
const OFF_GID_HI: usize = 0x7A;

/// Length of the embedded `i_block` array in bytes.
pub const I_BLOCK_LEN: usize = 60;

/// Minimum inode size — the rev-0 layout. Bytes past this offset
/// only exist on rev 1+ images.
pub const MIN_INODE_SIZE: u16 = 128;

// File-type bits (top 4 bits of i_mode). These mirror the POSIX
// S_IFMT family and the kernel's EXT4_FT_* values map onto them.
const S_IFMT: u16 = 0xF000;
const S_IFIFO: u16 = 0x1000;
const S_IFCHR: u16 = 0x2000;
const S_IFDIR: u16 = 0x4000;
const S_IFBLK: u16 = 0x6000;
const S_IFREG: u16 = 0x8000;
const S_IFLNK: u16 = 0xA000;
const S_IFSOCK: u16 = 0xC000;

/// Inode flag: this inode uses an extent tree (`i_block` opens with
/// an extent header), not the legacy block-pointer / indirect-block
/// scheme. The default for ext4-created files since the format was
/// finalised.
pub const INODE_FLAG_EXTENTS: u32 = 0x0008_0000;

/// Inode flag: file data lives inline in `i_block` and the extra
/// inode area, no data blocks allocated. Used for very small files
/// when the `INLINE_DATA` feature is enabled.
pub const INODE_FLAG_INLINE_DATA: u32 = 0x1000_0000;

/// Inode flag: file size is in 4 KiB units rather than 512-byte
/// sectors (combined with `i_blocks_high`). Affects how
/// `blocks_lo`/`blocks_hi` should be interpreted by higher layers.
pub const INODE_FLAG_HUGE_FILE: u32 = 0x0004_0000;

/// File-type discriminant extracted from the top 4 bits of i_mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeFileType {
    Fifo,
    CharacterDevice,
    Directory,
    BlockDevice,
    Regular,
    Symlink,
    Socket,
    /// Mode bits did not match any of the standard POSIX file
    /// types. Almost always corruption; preserved verbatim so
    /// callers can route on it instead of panicking.
    Unknown(u16),
}

/// Decoded ext4 inode — v0 fields. The 60-byte `i_block` array is
/// surfaced verbatim; downstream code interprets it as either an
/// extent header (when `INODE_FLAG_EXTENTS` is set) or as the
/// classic 12 direct + 1 indirect + 1 double + 1 triple block
/// pointer scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    /// Raw `i_mode` — top 4 bits are the file type, bottom 12 are
    /// POSIX permission bits + setuid/setgid/sticky.
    pub mode: u16,

    /// User ID. Combines lo + hi from the Linux OSD2 layout into
    /// a u32 — modern containers and namespaces routinely use UIDs
    /// above 65535.
    pub uid: u32,

    /// Group ID. Same hi+lo combining as `uid`.
    pub gid: u32,

    /// File size in bytes. Combines `i_size_lo` with `i_size_hi`.
    /// For regular files this is always correct on rev 1+ images.
    /// For directories, the kernel historically used the hi field
    /// as `i_dir_acl` (deprecated extended-attribute reference);
    /// modern `mkfs.ext4` images leave it zero so the combined
    /// value remains correct.
    pub size: u64,

    /// POSIX seconds-since-epoch. Extra-precision nanosecond bits
    /// (`i_atime_extra`) live past offset 128 and are not yet
    /// decoded.
    pub atime: u32,

    /// Status-change time (POSIX seconds).
    pub ctime: u32,

    /// Modification time (POSIX seconds).
    pub mtime: u32,

    /// Deletion time. Zero for live inodes; non-zero when the
    /// inode has been freed but not yet reused. Kernel uses this
    /// to find candidates for orphan-list cleanup.
    pub dtime: u32,

    /// Hard link count. Reaching zero in combination with a
    /// non-zero `dtime` means "deleted but bytes still on disk".
    pub links_count: u16,

    /// Block count low 32 bits — units depend on
    /// `INODE_FLAG_HUGE_FILE`: 512-byte sectors when clear,
    /// `block_size`-byte filesystem blocks when set.
    pub blocks_lo: u32,

    /// Block count high bits, extracted from the Linux OSD2
    /// `l_i_blocks_high` field. Higher layers combine with
    /// `blocks_lo` per the `HUGE_FILE` rule.
    pub blocks_hi: u16,

    /// `i_flags` — see [`INODE_FLAG_EXTENTS`],
    /// [`INODE_FLAG_INLINE_DATA`], [`INODE_FLAG_HUGE_FILE`].
    pub flags: u32,

    /// Raw 60-byte `i_block` array. Extent-tree decoding lands in
    /// the next slice; for now callers receive the bytes verbatim.
    pub block: [u8; I_BLOCK_LEN],

    /// `i_generation` — NFS handle stability nonce. The kernel
    /// bumps it on inode reuse; preserved here verbatim.
    pub generation: u32,

    /// File ACL block address (low 32 bits). Combines with
    /// `file_acl_hi` from OSD2 on 64-bit images. Zero means "no
    /// ACL block".
    pub file_acl_lo: u32,

    /// File ACL block address (high 16 bits, OSD2).
    pub file_acl_hi: u16,
}

impl Inode {
    /// Decode an inode from a buffer at the inode boundary. The
    /// caller must read at least `sb.inode_size` bytes; this does
    /// no IO. v0 reads the first 128 bytes only — extras past that
    /// offset are silently skipped.
    pub fn decode(buf: &[u8], sb: &Superblock) -> Result<Self, InodeDecodeError> {
        let inode_size = sb.inode_size as usize;
        if inode_size < MIN_INODE_SIZE as usize {
            return Err(InodeDecodeError::SuperblockInodeSizeTooSmall {
                size: sb.inode_size,
            });
        }
        if buf.len() < inode_size {
            return Err(InodeDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: inode_size,
            });
        }

        let uid_lo = read_u16(buf, OFF_UID_LO) as u32;
        let uid_hi = read_u16(buf, OFF_UID_HI) as u32;
        let gid_lo = read_u16(buf, OFF_GID_LO) as u32;
        let gid_hi = read_u16(buf, OFF_GID_HI) as u32;

        let size_lo = read_u32(buf, OFF_SIZE_LO) as u64;
        let size_hi = read_u32(buf, OFF_SIZE_HI) as u64;

        let mut block = [0u8; I_BLOCK_LEN];
        block.copy_from_slice(&buf[OFF_BLOCK..OFF_BLOCK + I_BLOCK_LEN]);

        Ok(Inode {
            mode: read_u16(buf, OFF_MODE),
            uid: (uid_hi << 16) | uid_lo,
            gid: (gid_hi << 16) | gid_lo,
            size: (size_hi << 32) | size_lo,
            atime: read_u32(buf, OFF_ATIME),
            ctime: read_u32(buf, OFF_CTIME),
            mtime: read_u32(buf, OFF_MTIME),
            dtime: read_u32(buf, OFF_DTIME),
            links_count: read_u16(buf, OFF_LINKS_COUNT),
            blocks_lo: read_u32(buf, OFF_BLOCKS_LO),
            blocks_hi: read_u16(buf, OFF_BLOCKS_HI),
            flags: read_u32(buf, OFF_FLAGS),
            block,
            generation: read_u32(buf, OFF_GENERATION),
            file_acl_lo: read_u32(buf, OFF_FILE_ACL_LO),
            file_acl_hi: read_u16(buf, OFF_FILE_ACL_HI),
        })
    }

    /// File-type discriminant from the top 4 bits of `i_mode`.
    pub fn file_type(&self) -> InodeFileType {
        match self.mode & S_IFMT {
            S_IFIFO => InodeFileType::Fifo,
            S_IFCHR => InodeFileType::CharacterDevice,
            S_IFDIR => InodeFileType::Directory,
            S_IFBLK => InodeFileType::BlockDevice,
            S_IFREG => InodeFileType::Regular,
            S_IFLNK => InodeFileType::Symlink,
            S_IFSOCK => InodeFileType::Socket,
            other => InodeFileType::Unknown(other),
        }
    }

    /// Convenience: true iff the file type is a directory.
    pub fn is_directory(&self) -> bool {
        matches!(self.file_type(), InodeFileType::Directory)
    }

    /// Convenience: true iff the file type is a regular file.
    pub fn is_regular(&self) -> bool {
        matches!(self.file_type(), InodeFileType::Regular)
    }

    /// Convenience: true iff the file type is a symlink.
    pub fn is_symlink(&self) -> bool {
        matches!(self.file_type(), InodeFileType::Symlink)
    }

    /// True iff `i_block` opens with an extent header (the modern
    /// ext4 layout) rather than the ext2/3 indirect-block scheme.
    pub fn uses_extents(&self) -> bool {
        self.flags & INODE_FLAG_EXTENTS != 0
    }

    /// True iff file content is stored inline in `i_block` and any
    /// extra inode area, no data blocks allocated.
    pub fn has_inline_data(&self) -> bool {
        self.flags & INODE_FLAG_INLINE_DATA != 0
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

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InodeDecodeError {
    #[error("superblock reports inode_size {size}, below the kernel minimum of 128")]
    SuperblockInodeSizeTooSmall { size: u16 },

    #[error("input too small to contain an inode: have {actual} bytes, expected {expected}")]
    InputTooSmall { actual: usize, expected: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::superblock::{Superblock, EXT4_MAGIC, SUPERBLOCK_SIZE};

    /// Build a Superblock with caller-controlled inode_size. All
    /// other fields are at their `buf_with(rev=1)` defaults — the
    /// inode decoder doesn't care about block_size, feature bits,
    /// or anything else for v0.
    fn make_sb_with_inode_size(inode_size: u16) -> Superblock {
        let mut buf = vec![0u8; SUPERBLOCK_SIZE];
        // s_log_block_size = 2 (4 KiB)
        buf[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes());
        // s_magic
        buf[0x38..0x3A].copy_from_slice(&EXT4_MAGIC.to_le_bytes());
        // s_rev_level = 1
        buf[0x4C..0x50].copy_from_slice(&1u32.to_le_bytes());
        // s_inode_size at 0x58
        buf[0x58..0x5A].copy_from_slice(&inode_size.to_le_bytes());
        Superblock::decode(&buf).unwrap()
    }

    fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Build a 128-byte zeroed inode buffer — caller pokes fields.
    fn empty_128() -> Vec<u8> {
        vec![0u8; 128]
    }

    /// Truncated buffer must surface a typed error, not panic.
    ///
    /// Bug it catches: a parser that reads field bytes before
    /// length-checking would panic when the caller passes a
    /// short buffer (e.g. partial disk read at end of inode
    /// table). The typed error lets the caller retry / report.
    #[test]
    fn test_decode_truncated_buffer_returns_input_too_small() {
        let sb = make_sb_with_inode_size(256);
        let buf = vec![0u8; 64];
        let err = Inode::decode(&buf, &sb).unwrap_err();
        assert_eq!(
            err,
            InodeDecodeError::InputTooSmall {
                actual: 64,
                expected: 256,
            }
        );
    }

    /// A superblock claiming `inode_size = 64` is corrupt — the
    /// decoder must refuse to operate.
    ///
    /// Bug it catches: silently accepting a sub-128 inode_size
    /// would make every field offset invalid (i_block alone lives
    /// at 0x28..0x64, past 64 bytes), corrupting every read.
    #[test]
    fn test_decode_with_subspec_inode_size_returns_superblock_error() {
        let sb = make_sb_with_inode_size(64);
        let buf = vec![0u8; 256];
        let err = Inode::decode(&buf, &sb).unwrap_err();
        assert_eq!(
            err,
            InodeDecodeError::SuperblockInodeSizeTooSmall { size: 64 }
        );
    }

    /// File size combines `i_size_lo` and `i_size_hi` into a u64.
    ///
    /// Bug it catches: a decoder that returns just `size_lo`
    /// reports a maximum file size of 4 GiB - 1, silently
    /// truncating sizes for regular files larger than that. Modern
    /// ext4 supports up to 16 TiB per file.
    #[test]
    fn test_decode_size_combines_hi_lo_into_u64() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u32(&mut buf, OFF_SIZE_LO, 0x0000_0001);
        write_u32(&mut buf, OFF_SIZE_HI, 0x0000_0002);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.size, 0x0000_0002_0000_0001);
    }

    /// UID combines lo + Linux OSD2 hi.
    ///
    /// Bug it catches: a decoder that ignores `l_i_uid_high` caps
    /// reported UIDs at 65535. Container runtimes routinely
    /// allocate UIDs above this (`subuid` mappings start at 100000
    /// on most distros), so misreading the field corrupts
    /// ownership for every containerised file.
    #[test]
    fn test_decode_uid_combines_lo_with_osd2_high_word() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u16(&mut buf, OFF_UID_LO, 0xABCD);
        write_u16(&mut buf, OFF_UID_HI, 0x0001);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.uid, 0x0001_ABCD);
    }

    /// GID combines lo + Linux OSD2 hi (same rule as UID).
    #[test]
    fn test_decode_gid_combines_lo_with_osd2_high_word() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u16(&mut buf, OFF_GID_LO, 0x1234);
        write_u16(&mut buf, OFF_GID_HI, 0x0005);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.gid, 0x0005_1234);
    }

    /// `i_mode = 0o40755` (S_IFDIR | 0o755) decodes to Directory.
    ///
    /// Bug it catches: a decoder that masks the wrong bits — e.g.
    /// using `mode & 0xF` instead of `mode & 0xF000` — would
    /// pick up permission bits as the type discriminator, mapping
    /// every directory and regular file to the same type.
    #[test]
    fn test_file_type_directory_from_mode_0o40755() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u16(&mut buf, OFF_MODE, 0o040755);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.file_type(), InodeFileType::Directory);
        assert!(inode.is_directory());
        assert!(!inode.is_regular());
    }

    /// `i_mode = 0o100644` (S_IFREG | 0o644) decodes to Regular.
    #[test]
    fn test_file_type_regular_from_mode_0o100644() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u16(&mut buf, OFF_MODE, 0o100644);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.file_type(), InodeFileType::Regular);
        assert!(inode.is_regular());
    }

    /// `i_mode = 0o120777` (S_IFLNK | 0o777) decodes to Symlink.
    #[test]
    fn test_file_type_symlink_from_mode_0o120777() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u16(&mut buf, OFF_MODE, 0o120777);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.file_type(), InodeFileType::Symlink);
        assert!(inode.is_symlink());
    }

    /// Unknown file-type bits surface as `Unknown(raw)` — caller
    /// can route on the typed variant.
    ///
    /// Bug it catches: a decoder that panics on unknown mode bits
    /// would fail every read on a corrupted inode. Returning
    /// `Unknown(bits)` lets callers decide: report and skip, or
    /// abort. Crashing isn't an option for a tool meant to recover
    /// data from broken images.
    #[test]
    fn test_file_type_unknown_bits_returns_unknown_variant() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        // 0xE000 — not any defined S_IFMT value.
        write_u16(&mut buf, OFF_MODE, 0xE000);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.file_type(), InodeFileType::Unknown(0xE000));
    }

    /// `INODE_FLAG_EXTENTS` triggers `uses_extents()`.
    ///
    /// Bug it catches: a decoder that ignores `i_flags` and
    /// always assumes legacy block-pointer layout would
    /// misinterpret every modern ext4 file's `i_block` array as a
    /// list of u32 block numbers, when in fact byte 0..2 is the
    /// extent header magic 0xF30A. Reading data using the wrong
    /// scheme produces garbage.
    #[test]
    fn test_uses_extents_set_when_flag_bit_present() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u32(&mut buf, OFF_FLAGS, INODE_FLAG_EXTENTS);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert!(inode.uses_extents());
        assert!(!inode.has_inline_data());
    }

    /// `INODE_FLAG_INLINE_DATA` triggers `has_inline_data()`.
    #[test]
    fn test_has_inline_data_set_when_flag_bit_present() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u32(&mut buf, OFF_FLAGS, INODE_FLAG_INLINE_DATA);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert!(inode.has_inline_data());
        assert!(!inode.uses_extents());
    }

    /// `i_block` bytes are surfaced verbatim (no endian flip, no
    /// reordering).
    ///
    /// Bug it catches: a decoder that reads `i_block` as 15 u32s
    /// and re-emits them as bytes would byte-swap the extent
    /// header magic 0xF30A on big-endian decoders compiled by
    /// accident, or scramble extent entries even on LE if the
    /// re-emit endian is wrong. Treating the field as opaque
    /// bytes is the only safe API at this layer; the extent
    /// decoder owns interpretation.
    #[test]
    fn test_block_field_bytes_round_trip_verbatim() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        // 0xF30A = ext4 extent magic (little-endian on disk).
        let pattern: [u8; I_BLOCK_LEN] = std::array::from_fn(|i| i as u8);
        buf[OFF_BLOCK..OFF_BLOCK + I_BLOCK_LEN].copy_from_slice(&pattern);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.block, pattern);
    }

    /// `dtime = 0` indicates a live inode.
    ///
    /// Bug it catches: a tool that displays "deleted" status from
    /// `dtime` alone (without the corollary `links_count == 0`
    /// check) would falsely flag every live inode as deleted on a
    /// freshly mounted filesystem where the field reads zero.
    /// Distinguishing zero from non-zero correctly is the
    /// minimum guarantee the decoder must provide.
    #[test]
    fn test_dtime_zero_for_live_inode() {
        let sb = make_sb_with_inode_size(128);
        let buf = empty_128();
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.dtime, 0);
    }

    /// `dtime` non-zero value round-trips for a deleted-but-not-
    /// yet-reused inode.
    #[test]
    fn test_dtime_nonzero_for_deleted_inode() {
        let sb = make_sb_with_inode_size(128);
        let mut buf = empty_128();
        write_u32(&mut buf, OFF_DTIME, 0x6000_0000);
        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.dtime, 0x6000_0000);
    }

    /// Anchor smoke test: a realistic regular-file inode round-
    /// trips through every load-bearing field in one shot.
    ///
    /// This is the suite's one anchor smoke test (per the
    /// per-file no-tautology rule) — it confirms field offsets
    /// are pairwise consistent with each other, so individual
    /// negative tests above can be trusted not to mask a
    /// systematic offset slip.
    #[test]
    fn test_decode_realistic_regular_file_inode_round_trips() {
        let sb = make_sb_with_inode_size(256);
        let mut buf = vec![0u8; 256];
        write_u16(&mut buf, OFF_MODE, 0o100644);
        write_u16(&mut buf, OFF_UID_LO, 1000);
        write_u16(&mut buf, OFF_GID_LO, 1000);
        write_u32(&mut buf, OFF_SIZE_LO, 4096);
        write_u32(&mut buf, OFF_ATIME, 0x6500_0000);
        write_u32(&mut buf, OFF_CTIME, 0x6500_0001);
        write_u32(&mut buf, OFF_MTIME, 0x6500_0002);
        write_u16(&mut buf, OFF_LINKS_COUNT, 1);
        write_u32(&mut buf, OFF_BLOCKS_LO, 8); // 8 sectors = 4 KiB
        write_u32(&mut buf, OFF_FLAGS, INODE_FLAG_EXTENTS);

        let inode = Inode::decode(&buf, &sb).unwrap();
        assert_eq!(inode.mode, 0o100644);
        assert_eq!(inode.file_type(), InodeFileType::Regular);
        assert_eq!(inode.uid, 1000);
        assert_eq!(inode.gid, 1000);
        assert_eq!(inode.size, 4096);
        assert_eq!(inode.atime, 0x6500_0000);
        assert_eq!(inode.ctime, 0x6500_0001);
        assert_eq!(inode.mtime, 0x6500_0002);
        assert_eq!(inode.dtime, 0);
        assert_eq!(inode.links_count, 1);
        assert_eq!(inode.blocks_lo, 8);
        assert!(inode.uses_extents());
        assert!(!inode.has_inline_data());
    }
}

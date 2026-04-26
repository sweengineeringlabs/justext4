//! Ext4 directory entries — variable-length records inside a
//! directory's data blocks.
//!
//! Wire format (`ext4_dir_entry_2`, kernel `fs/ext4/ext4.h`):
//!
//! ```text
//! 0  inode      u32  — referenced inode number; 0 means "unused"
//! 4  rec_len    u16  — total record length, padded to 4 bytes
//! 6  name_len   u8   — actual name byte count
//! 7  file_type  u8   — type discriminant; mirrors EXT4_FT_*
//! 8  name       u8 * name_len
//! ```
//!
//! v0 supports only the `_2` layout (split name_len + file_type).
//! The legacy `ext4_dir_entry` variant (16-bit name_len, no
//! file_type) was the format before the `FILETYPE` feature bit
//! shipped in 2002 and is essentially extinct in modern images;
//! support lands when a caller produces a real one.
//!
//! Hash-tree directories (`EXT4_INDEX_FL` on the directory inode)
//! repurpose blocks past the first into a tree-of-hash structure;
//! v0 walks linear directories only. The first block of a hash-
//! tree dir still contains "." and ".." in plain entries, so this
//! decoder gives correct top-of-dir results even on hash-tree
//! dirs — it just won't enumerate the rest.

/// Size of the `ext4_dir_entry_2` header (everything before the
/// name bytes).
pub const DIR_ENTRY_HEADER_SIZE: usize = 8;

/// Records on disk are padded so `rec_len` is always a multiple
/// of this. The kernel rejects mismatched alignment.
pub const DIR_ENTRY_ALIGNMENT: usize = 4;

/// Maximum `name_len` representable in the 8-bit field. Filenames
/// longer than this can't exist in an ext4 directory.
pub const DIR_ENTRY_NAME_MAX: u8 = 255;

/// File-type discriminant from the `file_type` byte. Mirrors the
/// kernel's `EXT4_FT_*` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirEntryFileType {
    /// `EXT4_FT_UNKNOWN` (0). The official "type not stored" value;
    /// callers must consult the referenced inode's `i_mode` for
    /// the real type.
    Unknown,
    RegularFile,
    Directory,
    CharacterDevice,
    BlockDevice,
    Fifo,
    Socket,
    Symlink,
    /// Values >= 8 are reserved by the kernel; any value seen
    /// here indicates either a kernel addition we haven't
    /// caught up with or directory-block corruption.
    Reserved(u8),
}

impl DirEntryFileType {
    /// Convert the raw byte from the on-disk `file_type` field.
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Unknown,
            1 => Self::RegularFile,
            2 => Self::Directory,
            3 => Self::CharacterDevice,
            4 => Self::BlockDevice,
            5 => Self::Fifo,
            6 => Self::Socket,
            7 => Self::Symlink,
            other => Self::Reserved(other),
        }
    }
}

/// Decoded directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Inode the entry references. Zero indicates an unused slot
    /// (the kernel sets this when removing an entry that can't be
    /// merged into its predecessor without re-balancing the
    /// block).
    pub inode: u32,

    /// On-disk record length, in bytes, including header + name +
    /// alignment padding. Stepping by `rec_len` is the canonical
    /// way to advance to the next entry within a block.
    pub rec_len: u16,

    /// Raw file_type byte. Use [`DirEntry::file_type`] for the
    /// typed discriminant.
    pub file_type_raw: u8,

    /// Name bytes — exactly `name_len` bytes verbatim from disk,
    /// no nul terminator. ext4 names are byte sequences; the
    /// kernel does not impose a charset (though most consumers
    /// expect UTF-8).
    pub name: Vec<u8>,
}

impl DirEntry {
    /// Decode a single entry from the start of `buf`. Returns the
    /// decoded entry; the caller advances the buffer by
    /// `entry.rec_len` to find the next one.
    pub fn decode(buf: &[u8]) -> Result<Self, DirEntryDecodeError> {
        if buf.len() < DIR_ENTRY_HEADER_SIZE {
            return Err(DirEntryDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: DIR_ENTRY_HEADER_SIZE,
            });
        }

        let inode = read_u32(buf, 0);
        let rec_len = read_u16(buf, 4);
        let name_len = buf[6];
        let file_type_raw = buf[7];

        if (rec_len as usize) < DIR_ENTRY_HEADER_SIZE {
            return Err(DirEntryDecodeError::RecLenBelowHeaderSize { rec_len });
        }

        if (rec_len as usize) % DIR_ENTRY_ALIGNMENT != 0 {
            return Err(DirEntryDecodeError::RecLenNotAligned { rec_len });
        }

        let name_end = DIR_ENTRY_HEADER_SIZE + name_len as usize;
        if name_end > rec_len as usize {
            return Err(DirEntryDecodeError::NameLenExceedsRecLen { name_len, rec_len });
        }
        if name_end > buf.len() {
            return Err(DirEntryDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: name_end,
            });
        }

        Ok(DirEntry {
            inode,
            rec_len,
            file_type_raw,
            name: buf[DIR_ENTRY_HEADER_SIZE..name_end].to_vec(),
        })
    }

    /// Typed file-type variant. Identical to
    /// `DirEntryFileType::from_byte(self.file_type_raw)`.
    pub fn file_type(&self) -> DirEntryFileType {
        DirEntryFileType::from_byte(self.file_type_raw)
    }

    /// Name as a UTF-8 `&str`, lossily replacing invalid byte
    /// sequences with U+FFFD. Returns a `Cow<str>` analogue —
    /// since the lossy path may allocate, callers wanting the
    /// strict version should match on
    /// `std::str::from_utf8(&entry.name)` directly.
    pub fn name_str_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.name)
    }

    /// True iff the entry is a tombstone left by a removal that
    /// the kernel couldn't merge into its predecessor.
    pub fn is_unused(&self) -> bool {
        self.inode == 0
    }

    /// Serialise the entry into the start of `buf`. Writes
    /// `rec_len` bytes total — header + name + alignment padding
    /// (zeroed). The caller is responsible for choosing
    /// `self.rec_len` such that subsequent entries land at the
    /// right offset; for the last entry of a block the kernel
    /// expects `rec_len` to absorb the rest of the block.
    pub fn encode_into(&self, buf: &mut [u8]) -> Result<(), DirEntryEncodeError> {
        let rec_len = self.rec_len as usize;
        if rec_len < DIR_ENTRY_HEADER_SIZE {
            return Err(DirEntryEncodeError::RecLenBelowHeaderSize {
                rec_len: self.rec_len,
            });
        }
        if rec_len % DIR_ENTRY_ALIGNMENT != 0 {
            return Err(DirEntryEncodeError::RecLenNotAligned {
                rec_len: self.rec_len,
            });
        }
        if self.name.len() > DIR_ENTRY_NAME_MAX as usize {
            return Err(DirEntryEncodeError::NameTooLong {
                name_len: self.name.len(),
            });
        }
        let name_end = DIR_ENTRY_HEADER_SIZE + self.name.len();
        if name_end > rec_len {
            return Err(DirEntryEncodeError::NameLenExceedsRecLen {
                name_len: self.name.len() as u8,
                rec_len: self.rec_len,
            });
        }
        if buf.len() < rec_len {
            return Err(DirEntryEncodeError::OutputTooSmall {
                actual: buf.len(),
                expected: rec_len,
            });
        }

        // Zero the full record so trailing pad bytes are
        // deterministic.
        buf[..rec_len].fill(0);

        write_u32(buf, 0, self.inode);
        write_u16(buf, 4, self.rec_len);
        buf[6] = self.name.len() as u8;
        buf[7] = self.file_type_raw;
        buf[DIR_ENTRY_HEADER_SIZE..name_end].copy_from_slice(&self.name);
        Ok(())
    }
}

/// Walk a directory data block, decoding every entry it contains.
///
/// The block is consumed in `rec_len` strides; `rec_len = 0`
/// corruption is rejected up-front to prevent infinite loops, and
/// each entry is bounds-checked against the remaining buffer.
pub fn decode_dir_block(buf: &[u8]) -> Result<Vec<DirEntry>, DirEntryDecodeError> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset < buf.len() {
        let remaining = &buf[offset..];
        if remaining.len() < DIR_ENTRY_HEADER_SIZE {
            // Tail of block too small to contain another header.
            // Modern filesystems pad the last entry's rec_len to
            // the block boundary, so reaching here means the block
            // didn't end on a clean record boundary.
            return Err(DirEntryDecodeError::TrailingBytesBelowHeaderSize {
                trailing: remaining.len(),
            });
        }
        let entry = DirEntry::decode(remaining)?;
        let stride = entry.rec_len as usize;
        entries.push(entry);
        offset += stride;
    }
    Ok(entries)
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

fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DirEntryEncodeError {
    #[error("output buffer too small: have {actual} bytes, expected {expected}")]
    OutputTooSmall { actual: usize, expected: usize },

    #[error("rec_len {rec_len} is below the 8-byte header")]
    RecLenBelowHeaderSize { rec_len: u16 },

    #[error("rec_len {rec_len} is not aligned to 4 bytes")]
    RecLenNotAligned { rec_len: u16 },

    #[error("name length {name_len} exceeds the 255-byte field maximum")]
    NameTooLong { name_len: usize },

    #[error("name_len {name_len} would extend past rec_len {rec_len}")]
    NameLenExceedsRecLen { name_len: u8, rec_len: u16 },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DirEntryDecodeError {
    #[error("input too small for a dir entry: have {actual} bytes, expected {expected}")]
    InputTooSmall { actual: usize, expected: usize },

    #[error(
        "rec_len {rec_len} is below the 8-byte header — would loop forever or read past header"
    )]
    RecLenBelowHeaderSize { rec_len: u16 },

    #[error("rec_len {rec_len} is not aligned to 4 bytes — kernel rejects mismatched alignment")]
    RecLenNotAligned { rec_len: u16 },

    #[error("name_len {name_len} would extend past rec_len {rec_len}")]
    NameLenExceedsRecLen { name_len: u8, rec_len: u16 },

    #[error("trailing {trailing} bytes after last entry — block didn't end on a record boundary")]
    TrailingBytesBelowHeaderSize { trailing: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    /// Write an entry into `buf` at `offset`. Pads `rec_len` to
    /// the next 4-byte multiple unless `rec_len_override` is set.
    fn write_entry(
        buf: &mut [u8],
        offset: usize,
        inode: u32,
        name: &[u8],
        file_type: u8,
        rec_len_override: Option<u16>,
    ) -> usize {
        let payload = DIR_ENTRY_HEADER_SIZE + name.len();
        let padded = payload.div_ceil(DIR_ENTRY_ALIGNMENT) * DIR_ENTRY_ALIGNMENT;
        let rec_len = rec_len_override.unwrap_or(padded as u16);
        write_u32(buf, offset, inode);
        write_u16(buf, offset + 4, rec_len);
        buf[offset + 6] = name.len() as u8;
        buf[offset + 7] = file_type;
        buf[offset + DIR_ENTRY_HEADER_SIZE..offset + DIR_ENTRY_HEADER_SIZE + name.len()]
            .copy_from_slice(name);
        rec_len as usize
    }

    /// Truncated header (< 8 bytes) yields a typed error rather
    /// than panicking.
    ///
    /// Bug it catches: a parser that indexes into the header
    /// without length-checking would panic on a partial block
    /// read at the tail of a directory file. Typed error lets
    /// the caller route on it.
    #[test]
    fn test_decode_truncated_header_returns_input_too_small() {
        let buf = vec![0u8; 4];
        let err = DirEntry::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryDecodeError::InputTooSmall {
                actual: 4,
                expected: DIR_ENTRY_HEADER_SIZE,
            }
        );
    }

    /// `rec_len = 0` is rejected — accepting it would cause
    /// `decode_dir_block` to loop forever advancing by zero.
    ///
    /// Bug it catches: the canonical infinite-loop bug in any
    /// variable-length-record parser. A corrupt or malicious
    /// directory block with a single zero-rec_len entry would
    /// hang the walker indefinitely without this check.
    #[test]
    fn test_decode_rec_len_zero_is_rejected() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 11); // inode
        write_u16(&mut buf, 4, 0); // rec_len = 0
        let err = DirEntry::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryDecodeError::RecLenBelowHeaderSize { rec_len: 0 }
        );
    }

    /// `rec_len < 8` is rejected — too small to contain the
    /// header.
    ///
    /// Bug it catches: a parser that only checks `rec_len > 0`
    /// would still walk past the buffer end on a `rec_len = 4`
    /// (smaller than the header itself), reading subsequent
    /// entries from misaligned offsets and corrupting the entire
    /// block walk.
    #[test]
    fn test_decode_rec_len_smaller_than_header_is_rejected() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 11);
        write_u16(&mut buf, 4, 4); // rec_len = 4 < 8
        let err = DirEntry::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryDecodeError::RecLenBelowHeaderSize { rec_len: 4 }
        );
    }

    /// `rec_len` not aligned to 4 bytes is rejected.
    ///
    /// Bug it catches: an unaligned rec_len misadvances the block
    /// walker, producing a chain of misaligned reads that all
    /// look mostly-plausible but have shifted-by-2-bytes inode
    /// numbers and name fields. The kernel would refuse to mount
    /// the FS on this; we reject early so callers see the same
    /// behaviour as the kernel does.
    #[test]
    fn test_decode_unaligned_rec_len_is_rejected() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 11);
        write_u16(&mut buf, 4, 10); // 10 % 4 != 0
        let err = DirEntry::decode(&buf).unwrap_err();
        assert_eq!(err, DirEntryDecodeError::RecLenNotAligned { rec_len: 10 });
    }

    /// `name_len` larger than `rec_len - 8` is rejected.
    ///
    /// Bug it catches: a corrupt directory block with name_len =
    /// 200 inside a rec_len = 16 entry would cause a parser to
    /// read 200 name bytes from past the entry boundary, leaking
    /// adjacent-entry data into this entry's name field.
    #[test]
    fn test_decode_name_len_exceeding_rec_len_is_rejected() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 11);
        write_u16(&mut buf, 4, 16);
        buf[6] = 200; // name_len = 200, but rec_len = 16
        buf[7] = 1;
        let err = DirEntry::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryDecodeError::NameLenExceedsRecLen {
                name_len: 200,
                rec_len: 16,
            }
        );
    }

    /// Realistic single entry decodes correctly: inode 12, name
    /// "hello.txt" (9 bytes), file_type RegularFile.
    ///
    /// Bug it catches: any field-offset slip in the header layout
    /// would surface here — e.g. confusing rec_len with name_len
    /// would round-trip 16 → 9 (or 9 → 16), both of which fail
    /// the assertion.
    #[test]
    fn test_decode_realistic_regular_file_entry_round_trips() {
        let mut buf = vec![0u8; 32];
        write_entry(&mut buf, 0, 12, b"hello.txt", 1, None);
        let entry = DirEntry::decode(&buf).unwrap();
        assert_eq!(entry.inode, 12);
        assert_eq!(entry.name, b"hello.txt");
        assert_eq!(entry.file_type(), DirEntryFileType::RegularFile);
        assert_eq!(entry.rec_len, 20); // 8 + 9 → padded to 20
        assert!(!entry.is_unused());
    }

    /// `file_type = 2` decodes to Directory.
    #[test]
    fn test_file_type_2_decodes_to_directory() {
        let mut buf = vec![0u8; 32];
        write_entry(&mut buf, 0, 11, b"subdir", 2, None);
        let entry = DirEntry::decode(&buf).unwrap();
        assert_eq!(entry.file_type(), DirEntryFileType::Directory);
    }

    /// `inode = 0` flags the entry as unused.
    ///
    /// Bug it catches: a directory walker that treats every entry
    /// as live would emit ghost entries (orphan filenames) for
    /// tombstones the kernel hasn't yet merged into the
    /// predecessor's rec_len.
    #[test]
    fn test_inode_zero_flags_entry_as_unused() {
        let mut buf = vec![0u8; 16];
        write_entry(&mut buf, 0, 0, b"x", 1, None);
        let entry = DirEntry::decode(&buf).unwrap();
        assert!(entry.is_unused());
    }

    /// Reserved file_type values surface as `Reserved(raw)`
    /// instead of panicking.
    ///
    /// Bug it catches: a `match` on file_type that panics on
    /// unknown values would crash on a corrupt block. Returning
    /// the typed `Reserved(raw)` lets the caller decide: skip,
    /// report, or reject.
    #[test]
    fn test_file_type_byte_above_7_returns_reserved_variant() {
        let mut buf = vec![0u8; 16];
        write_entry(&mut buf, 0, 12, b"x", 99, None);
        let entry = DirEntry::decode(&buf).unwrap();
        assert_eq!(entry.file_type(), DirEntryFileType::Reserved(99));
    }

    /// `decode_dir_block` walks two entries by stepping rec_len.
    ///
    /// Bug it catches: a walker that advances by a hard-coded
    /// step (e.g. `8 + name_len` without alignment padding) drifts
    /// out of phase with the on-disk records after one entry.
    /// Subsequent entries decode from misaligned offsets, all
    /// failing or returning garbage.
    #[test]
    fn test_decode_dir_block_walks_two_entries_by_rec_len() {
        // 64-byte block: two entries, last one padded to fill block.
        let mut buf = vec![0u8; 64];
        let stride = write_entry(&mut buf, 0, 12, b"a.txt", 1, None);
        // Second entry: rec_len padded to (64 - stride) so block
        // ends cleanly.
        let last_rec = 64 - stride;
        write_entry(&mut buf, stride, 13, b"b.txt", 1, Some(last_rec as u16));

        let entries = decode_dir_block(&buf).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, b"a.txt");
        assert_eq!(entries[0].inode, 12);
        assert_eq!(entries[1].name, b"b.txt");
        assert_eq!(entries[1].inode, 13);
    }

    /// `decode_dir_block` errors on a single zero-rec_len entry
    /// instead of looping forever.
    ///
    /// Bug it catches: the infinite-loop bug at the *iteration*
    /// layer — even if `DirEntry::decode` rejects rec_len=0, the
    /// walker must propagate that rejection rather than skip the
    /// entry and re-decode the same offset.
    #[test]
    fn test_decode_dir_block_with_zero_rec_len_does_not_loop() {
        let mut buf = vec![0u8; 16];
        write_u32(&mut buf, 0, 11);
        // rec_len = 0 — would hang a naive walker.
        let err = decode_dir_block(&buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryDecodeError::RecLenBelowHeaderSize { rec_len: 0 }
        );
    }

    /// Anchor smoke test — realistic root-directory shape: ".",
    /// "..", then a regular file entry. The third entry's
    /// rec_len absorbs the rest of the block (typical kernel
    /// layout).
    #[test]
    fn test_decode_dir_block_realistic_root_directory_layout() {
        let mut buf = vec![0u8; 64];
        let s1 = write_entry(&mut buf, 0, 2, b".", 2, Some(12));
        let s2 = write_entry(&mut buf, s1, 2, b"..", 2, Some(12));
        let off3 = s1 + s2;
        let last = (64 - off3) as u16;
        write_entry(&mut buf, off3, 11, b"lost+found", 2, Some(last));

        let entries = decode_dir_block(&buf).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, b".");
        assert_eq!(entries[0].file_type(), DirEntryFileType::Directory);
        assert_eq!(entries[1].name, b"..");
        assert_eq!(entries[2].name, b"lost+found");
        assert_eq!(entries[2].inode, 11);
    }

    /// Encode a single entry, decode it back, assert equality.
    /// Including padding bytes and a non-default file_type.
    #[test]
    fn test_dir_entry_encode_then_decode_round_trips() {
        let e1 = DirEntry {
            inode: 11,
            rec_len: 24, // 8 header + 9 name "hello.txt" → 17, padded to 20; pick 24
            file_type_raw: 1,
            name: b"hello.txt".to_vec(),
        };
        let mut buf = vec![0xAAu8; 32];
        e1.encode_into(&mut buf).unwrap();
        let e2 = DirEntry::decode(&buf).unwrap();
        assert_eq!(e1, e2);
        // Padding bytes must be zero so the on-disk form is
        // deterministic across encodes — the kernel relies on
        // this for checksum stability.
        assert!(buf[17..24].iter().all(|&b| b == 0));
    }

    /// Encoder rejects a rec_len smaller than the header.
    #[test]
    fn test_dir_entry_encode_rejects_rec_len_below_header() {
        let e = DirEntry {
            inode: 1,
            rec_len: 4,
            file_type_raw: 1,
            name: vec![],
        };
        let mut buf = vec![0u8; 16];
        let err = e.encode_into(&mut buf).unwrap_err();
        assert_eq!(
            err,
            DirEntryEncodeError::RecLenBelowHeaderSize { rec_len: 4 }
        );
    }

    /// Encoder rejects an unaligned rec_len that the decoder
    /// would reject.
    #[test]
    fn test_dir_entry_encode_rejects_unaligned_rec_len() {
        let e = DirEntry {
            inode: 1,
            rec_len: 10,
            file_type_raw: 1,
            name: vec![],
        };
        let mut buf = vec![0u8; 16];
        let err = e.encode_into(&mut buf).unwrap_err();
        assert_eq!(err, DirEntryEncodeError::RecLenNotAligned { rec_len: 10 });
    }
}

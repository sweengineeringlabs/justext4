//! Ext4 extent tree — the modern (post-ext3) layout for mapping
//! logical file blocks to physical disk blocks.
//!
//! An extent tree node is a 12-byte header followed by N entries,
//! each 12 bytes. The header's `eh_depth` tells you which kind of
//! entry follows:
//!
//! - depth = 0: entries are leaf [`Extent`] records — runs of
//!   contiguous physical blocks.
//! - depth > 0: entries are [`ExtentIndex`] records — pointers to
//!   the next-level node block.
//!
//! v0 scope: decode a single node from a byte buffer. Walking
//! depth > 0 trees requires reading the pointed-to block from disk;
//! that lives in the higher-level `swe_justext4_ext4` crate.
//!
//! Wire format reference: kernel `fs/ext4/ext4_extents.h`,
//! `struct ext4_extent_header` / `ext4_extent` / `ext4_extent_idx`.

/// Magic at the start of every extent header.
pub const EXTENT_HEADER_MAGIC: u16 = 0xF30A;

/// Size of the extent header on disk.
pub const EXTENT_HEADER_SIZE: usize = 12;

/// Size of an extent or extent-index entry.
pub const EXTENT_ENTRY_SIZE: usize = 12;

/// Threshold above which `ee_len` indicates an uninitialised
/// (preallocated but not yet written) extent. The kernel encodes
/// the uninit flag by adding 32768 to the run length; the decoded
/// run length is the original value with that offset subtracted.
const EXTENT_UNINIT_THRESHOLD: u16 = 32768;

/// Decoded extent header (12 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentHeader {
    /// Number of valid entries that follow the header.
    pub entries: u16,

    /// Maximum entries this node can hold. For inode-embedded
    /// headers (60-byte `i_block`) this is 4. For external blocks
    /// it depends on the filesystem's block size.
    pub max: u16,

    /// 0 for leaf nodes (`Extent` records follow); >0 for
    /// internal nodes (`ExtentIndex` records follow).
    pub depth: u16,

    /// Verbatim `eh_generation` — bumped by the kernel on writes
    /// for change detection.
    pub generation: u32,
}

impl ExtentHeader {
    /// Decode the 12-byte header at the start of `buf`. Validates
    /// the magic; the entries / max / depth fields are surfaced
    /// verbatim — callers may want to assert `entries <= max`
    /// themselves.
    pub fn decode(buf: &[u8]) -> Result<Self, ExtentDecodeError> {
        if buf.len() < EXTENT_HEADER_SIZE {
            return Err(ExtentDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: EXTENT_HEADER_SIZE,
            });
        }
        let magic = read_u16(buf, 0);
        if magic != EXTENT_HEADER_MAGIC {
            return Err(ExtentDecodeError::BadMagic { found: magic });
        }
        Ok(ExtentHeader {
            entries: read_u16(buf, 2),
            max: read_u16(buf, 4),
            depth: read_u16(buf, 6),
            generation: read_u32(buf, 8),
        })
    }

    /// True iff this header tags a leaf node (depth = 0). Leaf
    /// entries are [`Extent`]s.
    pub fn is_leaf(&self) -> bool {
        self.depth == 0
    }
}

/// Leaf-node entry: a contiguous run of physical blocks mapped to
/// a contiguous logical-block range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extent {
    /// First logical block this extent covers.
    pub logical_block: u32,

    /// Number of contiguous blocks. Maximum 32767 — values above
    /// that on disk encode the uninit flag and the real length is
    /// `ee_len - 32768`, surfaced via [`Extent::uninit`] and
    /// already adjusted in this field.
    pub len: u16,

    /// Physical block address (48-bit value, surfaced as u64).
    /// Combines `ee_start_hi` (16 bits) with `ee_start_lo` (32
    /// bits).
    pub physical_block: u64,

    /// True iff the kernel encoded this as a preallocated-but-
    /// not-yet-written run (`ee_len` was > 32768 on disk). Reads
    /// of an uninit extent return zero blocks; writes to one flip
    /// the flag once the data lands.
    pub uninit: bool,
}

impl Extent {
    /// Decode a single 12-byte leaf-extent entry.
    pub fn decode(buf: &[u8]) -> Result<Self, ExtentDecodeError> {
        if buf.len() < EXTENT_ENTRY_SIZE {
            return Err(ExtentDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: EXTENT_ENTRY_SIZE,
            });
        }
        let logical_block = read_u32(buf, 0);
        let raw_len = read_u16(buf, 4);
        let start_hi = read_u16(buf, 6) as u64;
        let start_lo = read_u32(buf, 8) as u64;

        let (len, uninit) = if raw_len > EXTENT_UNINIT_THRESHOLD {
            (raw_len - EXTENT_UNINIT_THRESHOLD, true)
        } else {
            (raw_len, false)
        };

        Ok(Extent {
            logical_block,
            len,
            physical_block: (start_hi << 32) | start_lo,
            uninit,
        })
    }
}

/// Internal-node entry: pointer to a next-level extent tree node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtentIndex {
    /// First logical block covered by the subtree this index
    /// points to.
    pub logical_block: u32,

    /// Physical block holding the next-level extent header.
    /// 48-bit value; combines `ei_leaf_hi` with `ei_leaf_lo`.
    pub leaf_block: u64,
}

impl ExtentIndex {
    /// Decode a single 12-byte internal-node entry.
    pub fn decode(buf: &[u8]) -> Result<Self, ExtentDecodeError> {
        if buf.len() < EXTENT_ENTRY_SIZE {
            return Err(ExtentDecodeError::InputTooSmall {
                actual: buf.len(),
                expected: EXTENT_ENTRY_SIZE,
            });
        }
        let leaf_lo = read_u32(buf, 4) as u64;
        let leaf_hi = read_u16(buf, 8) as u64;
        Ok(ExtentIndex {
            logical_block: read_u32(buf, 0),
            leaf_block: (leaf_hi << 32) | leaf_lo,
        })
    }
}

/// Decoded extent-tree node — header plus its body, with the body
/// type discriminated by depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtentNode {
    Leaf {
        header: ExtentHeader,
        extents: Vec<Extent>,
    },
    Internal {
        header: ExtentHeader,
        indices: Vec<ExtentIndex>,
    },
}

/// Decode a complete extent-tree node from a byte buffer (typically
/// the inode's `i_block` array, or one filesystem block read from
/// disk for non-leaf trees).
pub fn decode_extent_node(buf: &[u8]) -> Result<ExtentNode, ExtentDecodeError> {
    let header = ExtentHeader::decode(buf)?;
    let body_offset = EXTENT_HEADER_SIZE;
    let entries = header.entries as usize;
    let body_size = entries * EXTENT_ENTRY_SIZE;

    if buf.len() < body_offset + body_size {
        return Err(ExtentDecodeError::InputTooSmall {
            actual: buf.len(),
            expected: body_offset + body_size,
        });
    }

    if header.is_leaf() {
        let mut extents = Vec::with_capacity(entries);
        for i in 0..entries {
            let off = body_offset + i * EXTENT_ENTRY_SIZE;
            extents.push(Extent::decode(&buf[off..off + EXTENT_ENTRY_SIZE])?);
        }
        Ok(ExtentNode::Leaf { header, extents })
    } else {
        let mut indices = Vec::with_capacity(entries);
        for i in 0..entries {
            let off = body_offset + i * EXTENT_ENTRY_SIZE;
            indices.push(ExtentIndex::decode(&buf[off..off + EXTENT_ENTRY_SIZE])?);
        }
        Ok(ExtentNode::Internal { header, indices })
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
pub enum ExtentDecodeError {
    #[error("input too small: have {actual} bytes, expected {expected}")]
    InputTooSmall { actual: usize, expected: usize },

    #[error("bad extent header magic: found 0x{found:04x}, expected 0xf30a")]
    BadMagic { found: u16 },
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

    /// Build a header in the given buffer's first 12 bytes.
    fn write_header(buf: &mut [u8], entries: u16, max: u16, depth: u16) {
        write_u16(buf, 0, EXTENT_HEADER_MAGIC);
        write_u16(buf, 2, entries);
        write_u16(buf, 4, max);
        write_u16(buf, 6, depth);
        write_u32(buf, 8, 0);
    }

    /// Wrong magic must reject — extent trees with the wrong magic
    /// are corrupt and silently parsing them returns garbage.
    ///
    /// Bug it catches: a parser that skips magic validation would
    /// happily read entries from a buffer that's actually e.g. a
    /// directory block or indirect-block pointer table, returning
    /// physical-block addresses that point at random places on disk.
    #[test]
    fn test_decode_header_bad_magic_returns_bad_magic_error() {
        let mut buf = vec![0u8; 12];
        write_u16(&mut buf, 0, 0xDEAD);
        let err = ExtentHeader::decode(&buf).unwrap_err();
        assert_eq!(err, ExtentDecodeError::BadMagic { found: 0xDEAD });
    }

    /// Header read on a buffer shorter than 12 bytes returns a
    /// typed error, not a panic.
    ///
    /// Bug it catches: any path that reads `i_block[..12]` without
    /// length-checking would panic if a caller passes a 4-byte
    /// buffer (e.g. accidentally slicing too narrowly). Typed
    /// error means the caller can route on it.
    #[test]
    fn test_decode_header_truncated_returns_input_too_small() {
        let buf = vec![0u8; 4];
        let err = ExtentHeader::decode(&buf).unwrap_err();
        assert_eq!(
            err,
            ExtentDecodeError::InputTooSmall {
                actual: 4,
                expected: EXTENT_HEADER_SIZE,
            }
        );
    }

    /// Single leaf extent — the simple case: file at logical 0,
    /// 8 physical blocks at physical 100.
    ///
    /// Bug it catches: a parser that confuses ee_block (logical)
    /// with ee_start_lo (physical) — the most common shape error
    /// — would map the file's first block to wherever it found
    /// the field listed first. Reading the file then returns
    /// physical block 0's contents (often the boot sector / GDT)
    /// instead of the actual data.
    #[test]
    fn test_decode_single_leaf_extent_logical_to_physical() {
        let mut buf = vec![0u8; 24];
        write_u32(&mut buf, 0, 0); // ee_block (logical)
        write_u16(&mut buf, 4, 8); // ee_len
        write_u16(&mut buf, 6, 0); // ee_start_hi
        write_u32(&mut buf, 8, 100); // ee_start_lo
        let ext = Extent::decode(&buf[..12]).unwrap();
        assert_eq!(ext.logical_block, 0);
        assert_eq!(ext.len, 8);
        assert_eq!(ext.physical_block, 100);
        assert!(!ext.uninit);
    }

    /// `ee_len > 32768` decodes as `uninit = true` with
    /// `len = raw - 32768`.
    ///
    /// Bug it catches: a parser that returns `len` raw on
    /// preallocated-but-not-written extents would report runs of
    /// 32769+ blocks (longer than ext4 ever actually allocates —
    /// the per-extent maximum is 32768). Higher layers attempting
    /// to read the run hit a "real" data block followed by zeros,
    /// silently corrupting reads.
    #[test]
    fn test_decode_extent_uninit_flag_via_len_above_32768() {
        let mut buf = vec![0u8; 12];
        write_u32(&mut buf, 0, 0);
        // raw_len = 32768 + 100 → uninit, real len = 100.
        write_u16(&mut buf, 4, 32768 + 100);
        write_u32(&mut buf, 8, 200);
        let ext = Extent::decode(&buf).unwrap();
        assert_eq!(ext.len, 100);
        assert!(ext.uninit);
    }

    /// 48-bit physical block address combines `ee_start_hi` with
    /// `ee_start_lo`.
    ///
    /// Bug it catches: a parser that ignores `ee_start_hi` caps
    /// physical-block addresses at 2^32 - 1, breaking files
    /// allocated above the 16 TiB mark on large 64-bit
    /// filesystems. The hi field is u16, but it shifts in via
    /// `(hi as u64) << 32` to address 48 bits total.
    #[test]
    fn test_decode_extent_physical_block_combines_hi_lo() {
        let mut buf = vec![0u8; 12];
        write_u32(&mut buf, 0, 0);
        write_u16(&mut buf, 4, 1);
        write_u16(&mut buf, 6, 0x0001); // start_hi
        write_u32(&mut buf, 8, 0x0000_0042); // start_lo
        let ext = Extent::decode(&buf).unwrap();
        assert_eq!(ext.physical_block, 0x0001_0000_0042);
    }

    /// Internal-node `ExtentIndex` combines `ei_leaf_hi` with
    /// `ei_leaf_lo` to address the next-level node.
    ///
    /// Bug it catches: a parser that ignores `ei_leaf_hi` would
    /// follow the wrong physical block when descending the tree
    /// on a >16 TiB filesystem. Fields in the index entry are at
    /// different offsets than in a leaf extent (lo at +4, hi at
    /// +8) — easy to confuse with the leaf layout.
    #[test]
    fn test_decode_extent_index_leaf_block_combines_hi_lo() {
        let mut buf = vec![0u8; 12];
        write_u32(&mut buf, 0, 0); // ei_block
        write_u32(&mut buf, 4, 0x0000_0099); // ei_leaf_lo
        write_u16(&mut buf, 8, 0x0002); // ei_leaf_hi
        let idx = ExtentIndex::decode(&buf).unwrap();
        assert_eq!(idx.logical_block, 0);
        assert_eq!(idx.leaf_block, 0x0002_0000_0099);
    }

    /// `decode_extent_node` reports `InputTooSmall` when the
    /// header claims more entries than the buffer can hold.
    ///
    /// Bug it catches: a parser that trusts `eh_entries` blindly
    /// would OOB-read past the buffer end and either panic or
    /// return uninitialised memory contents as extent data. The
    /// length check up front turns this into a typed error.
    #[test]
    fn test_decode_node_truncated_body_returns_input_too_small() {
        // Header claims 4 entries (4 * 12 = 48 bytes of body) but
        // we give only 24 bytes total (12 header + 12 body).
        let mut buf = vec![0u8; 24];
        write_header(&mut buf, 4, 4, 0);
        let err = decode_extent_node(&buf).unwrap_err();
        assert_eq!(
            err,
            ExtentDecodeError::InputTooSmall {
                actual: 24,
                expected: 12 + 4 * 12,
            }
        );
    }

    /// Empty leaf node (entries=0) decodes successfully — sparse
    /// files with no allocated blocks have this shape.
    ///
    /// Bug it catches: a parser that errors on entries=0 would
    /// reject every empty-data file (e.g. a freshly-truncated
    /// regular file) as corrupt. The format permits zero entries;
    /// the read API just returns "no mapping" for any logical
    /// block.
    #[test]
    fn test_decode_node_empty_leaf_returns_empty_extents_vec() {
        let mut buf = vec![0u8; 12];
        write_header(&mut buf, 0, 4, 0);
        let node = decode_extent_node(&buf).unwrap();
        match node {
            ExtentNode::Leaf { header, extents } => {
                assert_eq!(header.entries, 0);
                assert!(extents.is_empty());
            }
            ExtentNode::Internal { .. } => panic!("expected Leaf for depth 0"),
        }
    }

    /// `decode_extent_node` discriminates Leaf vs Internal on
    /// `eh_depth`.
    ///
    /// Bug it catches: a parser that hard-codes "the body is
    /// always Extent records" would interpret index entries on a
    /// depth>0 node as physical-block runs, sending any read
    /// down to the wrong addresses (the index entry's leaf
    /// pointer would be misread as ee_start_lo). Tree walking
    /// fails for any file with >4 extents (the inode-embedded
    /// header maxes out at 4, anything bigger needs depth>0).
    #[test]
    fn test_decode_node_depth_1_returns_internal_with_indices() {
        let mut buf = vec![0u8; 36];
        write_header(&mut buf, 2, 4, 1);
        // Two index entries.
        write_u32(&mut buf, 12, 0); // ei_block
        write_u32(&mut buf, 16, 100); // ei_leaf_lo
        write_u16(&mut buf, 20, 0); // ei_leaf_hi
        write_u32(&mut buf, 24, 1000);
        write_u32(&mut buf, 28, 200);
        write_u16(&mut buf, 32, 0);
        let node = decode_extent_node(&buf).unwrap();
        match node {
            ExtentNode::Internal { header, indices } => {
                assert_eq!(header.depth, 1);
                assert_eq!(indices.len(), 2);
                assert_eq!(indices[0].logical_block, 0);
                assert_eq!(indices[0].leaf_block, 100);
                assert_eq!(indices[1].logical_block, 1000);
                assert_eq!(indices[1].leaf_block, 200);
            }
            ExtentNode::Leaf { .. } => panic!("expected Internal for depth 1"),
        }
    }

    /// Realistic-shape full decode: a leaf node with two extents
    /// covering a logically-fragmented but physically-contiguous
    /// file.
    ///
    /// Anchor smoke test for the module — confirms header-then-
    /// body offsets line up correctly so the per-field tests above
    /// can't be silently masked by a 4-byte slip in the body
    /// loop.
    #[test]
    fn test_decode_node_realistic_two_extent_leaf_round_trips() {
        let mut buf = vec![0u8; 12 + 24];
        write_header(&mut buf, 2, 4, 0);
        // Extent 0: logical 0..8 → physical 100..108.
        write_u32(&mut buf, 12, 0);
        write_u16(&mut buf, 16, 8);
        write_u16(&mut buf, 18, 0);
        write_u32(&mut buf, 20, 100);
        // Extent 1: logical 100..108 → physical 200..208.
        write_u32(&mut buf, 24, 100);
        write_u16(&mut buf, 28, 8);
        write_u16(&mut buf, 30, 0);
        write_u32(&mut buf, 32, 200);
        let node = decode_extent_node(&buf).unwrap();
        match node {
            ExtentNode::Leaf { header, extents } => {
                assert_eq!(header.entries, 2);
                assert!(header.is_leaf());
                assert_eq!(extents.len(), 2);
                assert_eq!(extents[0].logical_block, 0);
                assert_eq!(extents[0].physical_block, 100);
                assert_eq!(extents[1].logical_block, 100);
                assert_eq!(extents[1].physical_block, 200);
            }
            ExtentNode::Internal { .. } => panic!("expected Leaf"),
        }
    }
}

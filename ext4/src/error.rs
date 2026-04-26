//! Error type for the ext4 read API.

use spec::{GroupDescriptorDecodeError, InodeDecodeError, SuperblockDecodeError};

/// All failure modes the read API surfaces.
///
/// Wraps the spec-layer decode errors via `#[from]` so callers can
/// route on the originating layer. IO errors propagate verbatim.
#[derive(Debug, thiserror::Error)]
pub enum Ext4Error {
    /// IO failure reading from the underlying image source.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Superblock decode failed — magic mismatch, truncation, or
    /// invalid block-size shift.
    #[error("superblock decode: {0}")]
    Superblock(#[from] SuperblockDecodeError),

    /// Group descriptor decode failed.
    #[error("group descriptor decode: {0}")]
    GroupDescriptor(#[from] GroupDescriptorDecodeError),

    /// Inode decode failed.
    #[error("inode decode: {0}")]
    Inode(#[from] InodeDecodeError),

    /// Inode number is outside the valid range. Inode 0 is never
    /// valid (the kernel reserves it as the "no inode" sentinel);
    /// numbers above the superblock's `inodes_count` reference
    /// non-existent records.
    #[error("inode {inode} out of range [1, {max}]")]
    InodeOutOfRange { inode: u32, max: u32 },

    /// Superblock reports a corrupt layout that the read API
    /// can't proceed against — `blocks_per_group = 0` would mean
    /// no groups, `inodes_per_group = 0` would divide-by-zero
    /// during inode lookup.
    #[error("superblock layout invalid: {reason}")]
    InvalidLayout { reason: &'static str },
}

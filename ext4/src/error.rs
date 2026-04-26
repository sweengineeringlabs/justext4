//! Error type for the ext4 read API.

use spec::{
    DirEntryDecodeError, DirEntryEncodeError, ExtentDecodeError, ExtentEncodeError,
    GroupDescriptorDecodeError, GroupDescriptorEncodeError, InodeDecodeError, InodeEncodeError,
    SuperblockDecodeError, SuperblockEncodeError,
};

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

    /// Extent tree node decode failed.
    #[error("extent decode: {0}")]
    Extent(#[from] ExtentDecodeError),

    /// Directory entry decode failed.
    #[error("directory entry decode: {0}")]
    DirEntry(#[from] DirEntryDecodeError),

    /// Path resolution did not find a matching entry. Surfaces
    /// the missing component name so callers can report which
    /// part of the path was wrong.
    #[error("not found: {name:?}")]
    NotFound { name: Vec<u8> },

    /// A non-final path component referenced a non-directory
    /// inode — e.g. open_path("/etc/passwd/foo") where
    /// `/etc/passwd` is a regular file.
    #[error("not a directory: inode {inode}")]
    NotADirectory { inode: u32 },

    /// `unlink` was called on a directory inode. Directories
    /// have their own removal semantics (`rmdir`); unlink
    /// rejects them to avoid orphaning the dir's content.
    #[error("is a directory: inode {inode}")]
    IsADirectory { inode: u32 },

    /// A creation operation found an entry already at the target
    /// name. Surfaces the colliding name so callers can prompt
    /// the user or pick a different filename.
    #[error("already exists: {name:?}")]
    AlreadyExists { name: Vec<u8> },

    /// Allocator exhausted — no free inode or no contiguous
    /// block run of the requested length. The `what` field names
    /// which resource ran out.
    #[error("no space: {what}")]
    NoSpace { what: &'static str },

    /// A write operation can't proceed because the surrounding
    /// state is one v0 doesn't yet handle (e.g. directory full
    /// and would need a new data block, multi-group images for
    /// allocation). Documents the gap in the error itself so
    /// production code can route on it.
    #[error("unsupported in v0: {detail}")]
    UnsupportedV0 { detail: &'static str },

    /// Encode-side errors when writing structures back to bytes
    /// (used by `mkfs::format` and any future write paths).
    #[error("superblock encode: {0}")]
    SuperblockEncode(#[from] SuperblockEncodeError),

    #[error("group descriptor encode: {0}")]
    GroupDescriptorEncode(#[from] GroupDescriptorEncodeError),

    #[error("inode encode: {0}")]
    InodeEncode(#[from] InodeEncodeError),

    #[error("extent encode: {0}")]
    ExtentEncode(#[from] ExtentEncodeError),

    #[error("directory entry encode: {0}")]
    DirEntryEncode(#[from] DirEntryEncodeError),

    /// Extent walk requested on an inode that uses the legacy
    /// ext2/3 block-pointer scheme rather than ext4 extents.
    /// v0 only walks extent-based inodes; legacy support lands
    /// when a real consumer needs it.
    #[error("inode does not use extents (legacy block-pointer layout not supported)")]
    NotExtentBased,

    /// Extent tree depth exceeded the kernel's maximum (5). Either
    /// the image is corrupt or it was produced by a non-conforming
    /// builder.
    #[error("extent tree depth exceeded the maximum of {max}")]
    MaxExtentDepthExceeded { max: u16 },

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

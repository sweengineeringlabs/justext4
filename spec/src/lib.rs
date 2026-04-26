//! On-disk format types for ext4.
//!
//! This crate is a primitive: pure structs and decode/encode logic
//! over byte buffers, with no IO. Higher-level read/write APIs live
//! in `swe_justext4_ext4` (lands when the spec layer is firm).
//!
//! Reference: kernel `fs/ext4/ext4.h` plus the kernel.org ext4
//! on-disk layout doc. Field names mirror the kernel's `s_*` /
//! `bg_*` / `i_*` conventions where practical so cross-referencing
//! the kernel source is straightforward.

pub mod extent;
pub mod group_descriptor;
pub mod inode;
pub mod superblock;

pub use extent::{
    decode_extent_node, Extent, ExtentDecodeError, ExtentHeader, ExtentIndex, ExtentNode,
    EXTENT_ENTRY_SIZE, EXTENT_HEADER_MAGIC, EXTENT_HEADER_SIZE,
};
pub use group_descriptor::{
    GroupDescriptor, GroupDescriptorDecodeError, BG_FLAG_BLOCK_UNINIT, BG_FLAG_INODE_UNINIT,
    BG_FLAG_INODE_ZEROED,
};
pub use inode::{
    Inode, InodeDecodeError, InodeFileType, INODE_FLAG_EXTENTS, INODE_FLAG_HUGE_FILE,
    INODE_FLAG_INLINE_DATA, I_BLOCK_LEN, MIN_INODE_SIZE,
};
pub use superblock::{
    Superblock, SuperblockDecodeError, EXT4_MAGIC, FEATURE_INCOMPAT_64BIT, GDT_ENTRY_SIZE_32,
    SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE,
};

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

pub mod group_descriptor;
pub mod superblock;

pub use group_descriptor::{
    GroupDescriptor, GroupDescriptorDecodeError, BG_FLAG_BLOCK_UNINIT, BG_FLAG_INODE_UNINIT,
    BG_FLAG_INODE_ZEROED,
};
pub use superblock::{
    Superblock, SuperblockDecodeError, EXT4_MAGIC, FEATURE_INCOMPAT_64BIT, GDT_ENTRY_SIZE_32,
    SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE,
};

//! Read API over ext4 images.
//!
//! Open an image via [`Filesystem::open`], then ask for inodes by
//! number via [`Filesystem::read_inode`]. Higher-level path
//! resolution and file-content reads land in subsequent slices.
//!
//! Depends on [`spec`] for the on-disk format types — this crate
//! owns IO and arithmetic; `spec` owns byte layouts.

pub mod error;
pub mod filesystem;
pub mod mkfs;

pub use error::Ext4Error;
pub use filesystem::{Filesystem, ROOT_INODE};
pub use mkfs::{format, Config};

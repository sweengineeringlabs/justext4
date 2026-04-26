//! Integration tests for the inode-metadata mutation ops:
//! `chmod`, `chown`, `utime`. All three are read-modify-write of
//! a small set of inode fields; this file pins the bug-class each
//! op is supposed to prevent.
//!
//! The tests run against a real `format()`-produced image (not a
//! hand-rolled in-memory layout) so they catch any drift between
//! `mkfs::format` and the inode-mutation paths — e.g. a test
//! image whose root inode happens to look right won't reveal a
//! field-offset slip in `Inode::encode_into` or `decode`.

use std::io::Cursor;

use ext4::{format, Config, Ext4Error, Filesystem, METADATA_CTIME};

/// Build a fresh image in memory and return a Cursor wrapping it.
/// Bytes are owned by the cursor; drop drops the image.
fn fresh_image() -> Cursor<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf), &Config::default())
        .expect("format default config produces a valid image");
    Cursor::new(buf)
}

/// `chmod` replaces only the bottom 12 bits of the inode's mode;
/// the top 4 (file-type) bits are preserved.
///
/// Bug it catches: a `chmod` that assigns the caller's value
/// directly into `inode.mode` (without the file-type-preserving
/// mask) would clobber `S_IFREG` to zero, leaving the inode with
/// no recognisable file type. e2fsck flags such inodes as "Inode
/// N has no type" in pass 1; readers that route on `is_regular()`
/// would silently treat the file as `Unknown` and skip it.
#[test]
fn test_chmod_replaces_perm_bits_preserves_file_type() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    let inum = fs
        .create_file("/file.txt", b"contents")
        .expect("create_file");
    // Sanity: create_file uses 0o100644.
    let pre = fs.read_inode(inum).expect("read_inode pre");
    assert_eq!(
        pre.mode, 0o100644,
        "create_file should produce a regular file with 0o644 perms"
    );

    fs.chmod("/file.txt", 0o755).expect("chmod 0o755");

    let post = fs.read_inode(inum).expect("read_inode post");
    assert_eq!(
        post.mode, 0o100755,
        "expected S_IFREG (0o100000) | 0o755 = 0o100755, got {:o}",
        post.mode
    );
    assert!(post.is_regular(), "file type must still be regular");
    assert_eq!(
        post.ctime, METADATA_CTIME,
        "chmod must bump ctime per POSIX"
    );
}

/// `chmod` on a directory keeps the directory file-type bits
/// even when the caller passes a perm value that, if naively
/// assigned, would erase the dir bit.
///
/// Bug it catches: a `chmod` implementation that does
/// `inode.mode = mode` without preserving the file-type nibble
/// would turn a directory into a 0-type inode the kernel can't
/// classify. Readers walking the parent's dir entries would see
/// the dir entry's `file_type` byte still saying "directory",
/// then route on `inode.is_directory()` → false, and silently
/// skip what is now an orphan directory's children.
#[test]
fn test_chmod_directory_keeps_directory_type() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    fs.mkdir("/sub").expect("mkdir");
    let inum = fs.open_path("/sub").expect("open_path /sub");
    let pre = fs.read_inode(inum).expect("read_inode pre");
    assert!(pre.is_directory(), "mkdir should produce a directory");

    fs.chmod("/sub", 0o700).expect("chmod 0o700");

    let post = fs.read_inode(inum).expect("read_inode post");
    assert_eq!(
        post.mode, 0o040700,
        "expected S_IFDIR (0o040000) | 0o700 = 0o040700, got {:o}",
        post.mode
    );
    assert!(post.is_directory(), "still a directory after chmod");
}

/// `chown` writes both uid and gid; the values round-trip
/// through the encoder + decoder.
///
/// Bug it catches: a chown that mutates `inode.uid` without also
/// re-writing the inode would leave the on-disk uid byte-for-byte
/// unchanged. A re-read would surface the original uid (0) and
/// the test would fail. Specifically asserts `uid == 1000` rather
/// than `uid != 0` so the test fails noisily when the wrong
/// value gets written.
#[test]
fn test_chown_sets_uid_and_gid() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    let inum = fs.create_file("/file.txt", b"data").expect("create_file");
    let pre = fs.read_inode(inum).expect("read_inode pre");
    assert_eq!(pre.uid, 0, "create_file defaults uid to 0");
    assert_eq!(pre.gid, 0, "create_file defaults gid to 0");

    fs.chown("/file.txt", 1000, 1000).expect("chown");

    let post = fs.read_inode(inum).expect("read_inode post");
    assert_eq!(post.uid, 1000, "uid must be 1000 after chown");
    assert_eq!(post.gid, 1000, "gid must be 1000 after chown");
    assert_eq!(
        post.ctime, METADATA_CTIME,
        "chown must bump ctime per POSIX"
    );
}

/// `chown` with a uid above the OSD2-low limit (0xFFFF) round-
/// trips through the high-word encode + decode path.
///
/// Bug it catches: an inode encoder that only writes the low 16
/// bits of uid into `OFF_UID_LO` and forgets the high word at
/// `OFF_UID_HI` would silently truncate any uid above 65535. A
/// caller running with `unshare --user` and a high-uid mapping
/// (the namespace convention) would see their `chown(0x10000)`
/// land as `chown(0)` — the *root* uid — a security-relevant
/// confusion.
#[test]
fn test_chown_with_uid_above_65535_round_trips_through_osd2_high_word() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    let inum = fs.create_file("/file.txt", b"data").expect("create_file");

    let high = 0x10000_u32;
    fs.chown("/file.txt", high, high).expect("chown");

    let post = fs.read_inode(inum).expect("read_inode post");
    assert_eq!(
        post.uid, high,
        "uid 0x10000 must round-trip through OSD2 hi+lo (got {:#x})",
        post.uid
    );
    assert_eq!(
        post.gid, high,
        "gid 0x10000 must round-trip through OSD2 hi+lo (got {:#x})",
        post.gid
    );
}

/// `utime` sets both atime and mtime; the values round-trip and
/// ctime is bumped to METADATA_CTIME (POSIX rule: every inode
/// mutation, even one that touches only access times, updates
/// ctime).
///
/// Bug it catches: a utime that swaps atime and mtime (a common
/// off-by-one when copy-pasting field assignments) would surface
/// here when `atime != mtime`. Also catches a utime that forgets
/// the ctime bump — a real-world security issue, since many
/// audit tools rely on ctime to detect tampering.
#[test]
fn test_utime_sets_atime_and_mtime() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    let inum = fs.create_file("/file.txt", b"data").expect("create_file");

    let atime = 1_700_000_000_u32; // distinct values so a swap surfaces
    let mtime = 1_700_001_000_u32;
    fs.utime("/file.txt", atime, mtime).expect("utime");

    let post = fs.read_inode(inum).expect("read_inode post");
    assert_eq!(post.atime, atime, "atime round-trip");
    assert_eq!(post.mtime, mtime, "mtime round-trip");
    assert_eq!(
        post.ctime, METADATA_CTIME,
        "utime must still bump ctime per POSIX"
    );
}

/// `chmod` on a path that doesn't resolve returns `NotFound`
/// (propagated from `open_path`).
///
/// Bug it catches: a method that swallows the resolution error
/// (e.g. `if let Ok(num) = self.open_path(path)`) and silently
/// no-ops would leave the caller thinking the chmod succeeded
/// while nothing on disk changed. The typed error is the
/// load-bearing signal that the path was wrong.
#[test]
fn test_chmod_missing_path_returns_not_found() {
    let mut fs = Filesystem::open(fresh_image()).expect("open");
    let err = fs.chmod("/does-not-exist", 0o755).unwrap_err();
    match err {
        Ext4Error::NotFound { name } => {
            assert_eq!(name, b"does-not-exist", "NotFound carries the missing name");
        }
        other => panic!("expected NotFound, got {other:?}"),
    }
}

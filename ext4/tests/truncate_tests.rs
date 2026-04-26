//! Integration tests for `Filesystem::truncate`.
//!
//! Each test names the bug it catches in its doc comment, per the
//! project convention. Lives in its own integration test file
//! (rather than as `#[cfg(test)]` next to the impl) so the
//! truncate work doesn't collide with parallel agents touching
//! `ext4/src/mkfs.rs` or other crates.

use std::io::Cursor;

use ext4::{format, Config, Filesystem};
use spec::{decode_extent_node, ExtentNode};

/// Helpers — build an in-memory image and re-open it as a
/// `Filesystem<Cursor<Vec<u8>>>`. Done as a function returning the
/// `Filesystem` so each test starts fresh and isolated.
fn fresh_image() -> Filesystem<Cursor<Vec<u8>>> {
    let mut buf: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf), &Config::default()).unwrap();
    Filesystem::open(Cursor::new(buf)).unwrap()
}

/// Truncating a 2-block file down to 0 must:
///   1. Free both data blocks (the block-bitmap free count rises
///      by 2),
///   2. Set inode.size to 0 and blocks_lo to 0,
///   3. Make `read_file` return an empty Vec.
///
/// **Bug it catches:** a shrink path that updates the size field
/// but forgets to call `free_blocks` leaks the data blocks.
/// e2fsck would flag "Free blocks count wrong"; this test catches
/// the same drift via the in-memory free-counts before any e2fsck
/// run is needed.
#[test]
fn test_truncate_shrink_to_zero_frees_all_blocks() {
    let mut fs = fresh_image();
    let block_size = fs.superblock().block_size as usize;

    // Two-block payload — block_size + 1 byte forces num_blocks=2.
    let payload = vec![0xAB; block_size + 1];
    let inode_num = fs.create_file("/twoblock.bin", &payload).unwrap();

    let free_blocks_before = fs.superblock().free_blocks_count;

    fs.truncate("/twoblock.bin", 0).unwrap();

    let free_blocks_after = fs.superblock().free_blocks_count;
    assert_eq!(
        free_blocks_after - free_blocks_before,
        2,
        "expected 2 blocks to be freed, got {}",
        free_blocks_after - free_blocks_before
    );

    let inode = fs.read_inode(inode_num).unwrap();
    assert_eq!(inode.size, 0, "size must be 0 after shrink-to-zero");
    assert_eq!(
        inode.blocks_lo, 0,
        "blocks_lo must be 0 after shrink-to-zero"
    );

    let data = fs.read_file(&inode).unwrap();
    assert!(
        data.is_empty(),
        "read_file must return empty after shrink-to-zero, got {} bytes",
        data.len()
    );

    // The leaf extent header should now report zero entries.
    let node = decode_extent_node(&inode.block).unwrap();
    match node {
        ExtentNode::Leaf { header, extents } => {
            assert_eq!(header.entries, 0, "leaf must have 0 entries");
            assert!(extents.is_empty(), "leaf extents must be empty");
        }
        ExtentNode::Internal { .. } => {
            panic!("create_file produces depth-0 trees; got Internal");
        }
    }
}

/// Truncating a 2-block file to a byte offset inside the FIRST
/// block must:
///   1. Keep the first physical block,
///   2. Free the second physical block (1 block freed total),
///   3. Set inode.size to the new (mid-block) value,
///   4. Surface the original first-block bytes via `read_file`,
///      truncated to the new size.
///
/// **Bug it catches:** a shrink path that always frees ALL extent
/// blocks (instead of partial-shrinking the straddling extent)
/// would corrupt the file by freeing the surviving first block.
/// A path that frees nothing (the inverse bug) would leak the
/// second block.
#[test]
fn test_truncate_shrink_to_partial_block_keeps_remaining_blocks() {
    let mut fs = fresh_image();
    let block_size = fs.superblock().block_size as usize;

    // Two-block payload with a recognisable first-block pattern so
    // we can prove the prefix data survived the shrink.
    let mut payload = vec![0u8; block_size + 100];
    for (i, b) in payload.iter_mut().enumerate() {
        *b = (i & 0xFF) as u8;
    }
    let inode_num = fs.create_file("/partial.bin", &payload).unwrap();

    let free_blocks_before = fs.superblock().free_blocks_count;

    // Cut off mid-first-block.
    let new_size = (block_size as u64) / 2;
    fs.truncate("/partial.bin", new_size).unwrap();

    let free_blocks_after = fs.superblock().free_blocks_count;
    assert_eq!(
        free_blocks_after - free_blocks_before,
        1,
        "expected exactly 1 block to be freed, got {}",
        free_blocks_after - free_blocks_before
    );

    let inode = fs.read_inode(inode_num).unwrap();
    assert_eq!(inode.size, new_size, "size must equal new_size");

    // blocks_lo is in 512-byte sectors per OSD2 convention.
    let expected_sectors = (block_size as u32) / 512;
    assert_eq!(
        inode.blocks_lo, expected_sectors,
        "blocks_lo must reflect 1 surviving block"
    );

    let data = fs.read_file(&inode).unwrap();
    assert_eq!(
        data.len(),
        new_size as usize,
        "read_file output must match new size"
    );
    // First half of the original payload survived.
    assert_eq!(
        data,
        payload[..new_size as usize],
        "surviving prefix bytes must match original payload"
    );
}

/// Growing a 1-block file from 100 bytes up to 1000 bytes (still
/// within the single allocated block) must:
///   1. Succeed (no UnsupportedV0 — fits in existing capacity),
///   2. Bump inode.size to 1000,
///   3. Have `read_file` return the original bytes followed by
///      zeros for the gap (kernel "read past EOF returns zero"
///      semantics; create_file pads the tail of the data block
///      with zeros so this falls out naturally).
///
/// **Bug it catches:** a grow path that always rejects (treating
/// any new_size > inode.size as needing allocation) refuses
/// legitimate grows that still fit in the existing tail. A grow
/// path that walks the wrong capacity calculation (e.g. uses
/// blocks_lo without the 512-byte conversion) under-counts and
/// rejects.
#[test]
fn test_truncate_grow_within_existing_blocks_succeeds() {
    let mut fs = fresh_image();
    let block_size = fs.superblock().block_size as u64;

    // Single-block file with content shorter than the block.
    let payload = vec![0xCD; 100];
    let inode_num = fs.create_file("/short.bin", &payload).unwrap();

    // Grow to 1000 bytes — still well below block_size (4 KiB
    // default), so no new allocation needed.
    let new_size = 1000u64;
    assert!(
        new_size < block_size,
        "test setup: new_size must fit in block"
    );

    fs.truncate("/short.bin", new_size).unwrap();

    let inode = fs.read_inode(inode_num).unwrap();
    assert_eq!(inode.size, new_size, "size must be bumped to new_size");

    let data = fs.read_file(&inode).unwrap();
    assert_eq!(
        data.len(),
        new_size as usize,
        "read length matches new size"
    );
    assert_eq!(&data[..100], &payload[..], "original bytes preserved");
    // The gap [100, 1000) reads as zeros — create_file zero-pads
    // the tail of the allocated block, so the kernel's
    // "read-past-old-EOF returns zero" semantics fall out.
    assert!(
        data[100..].iter().all(|&b| b == 0),
        "grown region must read as zeros"
    );
}

/// Growing a 1-block file beyond the byte-end of its existing
/// block must return `Ext4Error::UnsupportedV0`. v0 doesn't yet
/// allocate new blocks for sparse-grow; surfacing the limit
/// explicitly lets callers route on it (or fall back to
/// `unlink + create_file` with the larger payload).
///
/// **Bug it catches:** a grow path that silently allocates a new
/// block (without wiring the extent + bitmap correctly) would
/// produce an inconsistent image — wrong free counts, dangling
/// extent. A path that lets the size bump succeed without
/// allocating leaves the file claiming bytes it can't read,
/// which `read_file` would fault on or return garbage for.
#[test]
fn test_truncate_grow_requiring_new_block_returns_unsupported_v0() {
    let mut fs = fresh_image();
    let block_size = fs.superblock().block_size as u64;

    let payload = vec![0xEE; 100];
    fs.create_file("/needs_more.bin", &payload).unwrap();

    // Grow to 5000 bytes — exceeds the default 4 KiB block size
    // and therefore the file's currently-allocated capacity.
    let new_size = block_size + 1000; // > 1 block of capacity.

    let err = fs
        .truncate("/needs_more.bin", new_size)
        .expect_err("grow past allocated capacity must error");
    match err {
        ext4::Ext4Error::UnsupportedV0 { detail } => {
            assert!(
                detail.contains("new block allocation"),
                "error detail must name the missing capability, got: {detail:?}"
            );
        }
        other => panic!("expected UnsupportedV0, got {other:?}"),
    }
}

/// Calling `truncate` on a directory inode must return
/// `NotARegularFile`. Directories have their own removal
/// semantics (`rmdir`); resizing them in-place would corrupt
/// the directory-entry layout and orphan child inodes.
///
/// **Bug it catches:** a `truncate` that doesn't gate on
/// `is_regular()` would let a caller shrink a directory's
/// `i_block` to zero, leaking every inode the dir referenced and
/// producing a fsck-rejected image. POSIX `truncate(2)` returns
/// `EISDIR` for the directory case.
#[test]
fn test_truncate_directory_returns_not_a_regular_file() {
    let mut fs = fresh_image();

    let dir_inode = fs.mkdir("/sub").unwrap();

    let err = fs
        .truncate("/sub", 0)
        .expect_err("truncate on a directory must error");
    match err {
        ext4::Ext4Error::NotARegularFile { inode } => {
            assert_eq!(
                inode, dir_inode,
                "error must surface the offending dir's inode"
            );
        }
        other => panic!("expected NotARegularFile, got {other:?}"),
    }
}

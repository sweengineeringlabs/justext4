//! Integration tests for `Filesystem::rename`.
//!
//! Each test names the bug it catches in its docstring. The
//! pattern mirrors the existing write-path tests: build a fresh
//! image via `mkfs::format`, drive the API on top of an in-memory
//! `Cursor`, then read back via the open API to assert the
//! resulting state.

use std::io::Cursor;

use ext4::{format, Config, Ext4Error, Filesystem};

/// Build a fresh writable image into an in-memory cursor.
///
/// Returning a `Cursor<Vec<u8>>` keeps the tests self-contained —
/// no tempfiles, no IO ordering games, and the same API the
/// existing in-process tests use.
fn fresh_fs() -> Filesystem<Cursor<Vec<u8>>> {
    let mut buf: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf), &Config::default()).unwrap();
    Filesystem::open(Cursor::new(buf)).unwrap()
}

/// Rename within the same parent directory: the source goes away,
/// the destination appears, and the file's content is preserved
/// (the inode is the same, only the dir entry pointing at it
/// moved).
///
/// Bug it catches: a rename that deletes the source's inode
/// instead of just removing the dir entry would lose the file's
/// contents. A rename that adds the new entry but forgets to
/// remove the old one would leave a hard-linked alias (and break
/// the link-count invariant). A rename that points the new entry
/// at a stale inode number would surface here as "file not
/// reachable via new name".
#[test]
fn test_rename_regular_file_within_same_directory() {
    let mut fs = fresh_fs();
    let payload = b"hello rename";
    fs.create_file("/before.txt", payload).unwrap();
    let original_inode = fs.open_path("/before.txt").unwrap();

    fs.rename("/before.txt", "/after.txt").unwrap();

    // Old name no longer resolves.
    let err = fs.open_path("/before.txt").unwrap_err();
    assert!(
        matches!(err, Ext4Error::NotFound { ref name } if name == b"before.txt"),
        "expected NotFound for old name, got {err:?}"
    );

    // New name resolves to the SAME inode (not a copy).
    let after_inode = fs.open_path("/after.txt").unwrap();
    assert_eq!(
        after_inode, original_inode,
        "rename should preserve the inode, only the dir entry moves"
    );

    // Content is preserved end-to-end.
    let inode = fs.read_inode(after_inode).unwrap();
    let content = fs.read_file(&inode).unwrap();
    assert_eq!(content, payload);
}

/// Rename a regular file from `/` into `/sub`. After the rename,
/// the file is reachable via `/sub/file` and not via `/file`.
///
/// Bug it catches: a rename that doesn't insert into the new
/// parent's data block at all would surface here as "new path
/// not found". A rename that inserts but with the wrong
/// `file_type_raw` byte would still resolve via path walking
/// (path walk reads the dir entry and chases by inode number) —
/// but e2fsck's pass 2 catches the mismatch. The test below
/// proves resolution; the e2fsck regression catches the type byte.
#[test]
fn test_rename_regular_file_to_different_directory() {
    let mut fs = fresh_fs();
    fs.create_file("/file", b"contents").unwrap();
    fs.mkdir("/sub").unwrap();
    let original_inode = fs.open_path("/file").unwrap();

    fs.rename("/file", "/sub/file").unwrap();

    // Old path is gone.
    assert!(matches!(
        fs.open_path("/file").unwrap_err(),
        Ext4Error::NotFound { .. }
    ));

    // New path resolves to the same inode and same content.
    let new_inode = fs.open_path("/sub/file").unwrap();
    assert_eq!(new_inode, original_inode);
    let inode = fs.read_inode(new_inode).unwrap();
    assert_eq!(fs.read_file(&inode).unwrap(), b"contents");
}

/// Rename a directory across directories: the moved subtree's
/// `..` entry must point at the new parent, and the parents'
/// `links_count` fields must transfer the contribution.
///
/// Layout: `mkdir /a; mkdir /b; mkdir /a/x; rename /a/x → /b/x`.
///
/// Bug it catches: a rename that doesn't update the moved dir's
/// `..` entry would have `/b/x/..` still pointing at `/a`'s inode
/// — e2fsck's pass 3 would flag this as "directory's `..` doesn't
/// agree with its location" and the kernel would refuse to chdir
/// out of `/b/x` correctly. A rename that doesn't transfer the
/// `links_count` between parents would have `/a` over-counted
/// (still claiming a child it doesn't have) and `/b` under-
/// counted; e2fsck's pass 4 flags both as "Reference count wrong".
#[test]
fn test_rename_directory_across_dirs_updates_dotdot_and_links_counts() {
    let mut fs = fresh_fs();
    fs.mkdir("/a").unwrap();
    fs.mkdir("/b").unwrap();
    fs.mkdir("/a/x").unwrap();

    // Capture pre-rename state for delta assertions.
    let a_inode_num = fs.open_path("/a").unwrap();
    let b_inode_num = fs.open_path("/b").unwrap();
    let x_inode_num = fs.open_path("/a/x").unwrap();
    let a_links_before = fs.read_inode(a_inode_num).unwrap().links_count;
    let b_links_before = fs.read_inode(b_inode_num).unwrap().links_count;
    let x_links_before = fs.read_inode(x_inode_num).unwrap().links_count;

    fs.rename("/a/x", "/b/x").unwrap();

    // Old path is gone, new path exists, and it's the same inode.
    assert!(matches!(
        fs.open_path("/a/x").unwrap_err(),
        Ext4Error::NotFound { .. }
    ));
    let moved_num = fs.open_path("/b/x").unwrap();
    assert_eq!(moved_num, x_inode_num, "directory inode should not change");

    // `..` inside the moved dir now points at /b's inode, not /a's.
    let moved_inode = fs.read_inode(moved_num).unwrap();
    let entries = fs.read_dir(&moved_inode).unwrap();
    let dotdot = entries
        .iter()
        .find(|e| !e.is_unused() && e.name == b"..")
        .expect("moved directory must still have a `..` entry");
    assert_eq!(
        dotdot.inode, b_inode_num,
        "moved dir's `..` must point at the new parent"
    );

    // Parents' link-count deltas: /a went down by 1, /b went up by 1.
    let a_links_after = fs.read_inode(a_inode_num).unwrap().links_count;
    let b_links_after = fs.read_inode(b_inode_num).unwrap().links_count;
    assert_eq!(
        a_links_after,
        a_links_before - 1,
        "old parent /a should lose 1 link (the moved child's `..` no longer points here)"
    );
    assert_eq!(
        b_links_after,
        b_links_before + 1,
        "new parent /b should gain 1 link (the moved child's `..` now points here)"
    );

    // The moved dir's own links_count is unchanged — `.` + parent's
    // entry = 2; the move doesn't alter that.
    let x_links_after = fs.read_inode(moved_num).unwrap().links_count;
    assert_eq!(
        x_links_after, x_links_before,
        "moved directory's own links_count should not change"
    );
}

/// POSIX `rename(file_a, file_b)` overwrites the destination when
/// both are regular files. The destination's inode + blocks are
/// freed; the source's inode is now reachable under the
/// destination's name.
///
/// Bug it catches: a rename that refuses to overwrite would block
/// `mv a b` semantics — POSIX explicitly requires overwriting an
/// existing regular-file destination. A rename that adds the new
/// entry without unlinking the old destination first would leak
/// the destination's inode + blocks (e2fsck flags as orphaned
/// inode in pass 1). A rename that unlinks the destination but
/// then fails to re-point the dir entry would lose the source
/// too — both names would be gone.
#[test]
fn test_rename_overwrites_existing_regular_file_target() {
    let mut fs = fresh_fs();
    fs.create_file("/a", b"first").unwrap();
    fs.create_file("/b", b"second").unwrap();

    let a_inode_num = fs.open_path("/a").unwrap();
    let b_inode_num_before = fs.open_path("/b").unwrap();
    assert_ne!(a_inode_num, b_inode_num_before);

    fs.rename("/a", "/b").unwrap();

    // /a is gone.
    assert!(matches!(
        fs.open_path("/a").unwrap_err(),
        Ext4Error::NotFound { .. }
    ));
    // /b now points at what was /a's inode (so its content is "first").
    let b_inode_num_after = fs.open_path("/b").unwrap();
    assert_eq!(
        b_inode_num_after, a_inode_num,
        "destination should now point at the source's inode"
    );
    let inode = fs.read_inode(b_inode_num_after).unwrap();
    assert_eq!(fs.read_file(&inode).unwrap(), b"first");

    // The original /b's inode is freed: links_count = 0 and dtime
    // is non-zero (the unlink-deleted shape).
    let old_dst = fs.read_inode(b_inode_num_before).unwrap();
    assert_eq!(
        old_dst.links_count, 0,
        "overwritten dst inode must be freed"
    );
    assert_ne!(
        old_dst.dtime, 0,
        "overwritten dst inode must have dtime set"
    );
}

/// `rename(file, existing_dir)` returns AlreadyExists. POSIX
/// returns EEXIST or EISDIR for this case; v0 maps both to
/// AlreadyExists. The directory must NOT be removed and the
/// source must NOT be moved.
///
/// Bug it catches: a rename that overwrites a directory would
/// orphan everything inside it (including `lost+found` if the
/// user got unlucky with paths) and corrupt the parent's
/// links_count. The kernel guards against this; we must too.
#[test]
fn test_rename_to_existing_directory_returns_already_exists() {
    let mut fs = fresh_fs();
    fs.create_file("/a", b"payload").unwrap();
    fs.mkdir("/b").unwrap();

    let a_inode_before = fs.open_path("/a").unwrap();
    let b_inode_before = fs.open_path("/b").unwrap();

    let err = fs.rename("/a", "/b").unwrap_err();
    assert!(
        matches!(err, Ext4Error::AlreadyExists { ref name } if name == b"b"),
        "expected AlreadyExists, got {err:?}"
    );

    // Both /a and /b still exist and point at their original inodes.
    assert_eq!(fs.open_path("/a").unwrap(), a_inode_before);
    assert_eq!(fs.open_path("/b").unwrap(), b_inode_before);
}

/// `rename(missing, anything)` returns NotFound on the source
/// name — distinct from AlreadyExists or InvalidLayout so callers
/// can surface the right diagnostic to users.
///
/// Bug it catches: a rename that returns a generic error or
/// silently no-ops on a missing source would leave a CLI like
/// `mv` unable to report which path was wrong. Routing on a
/// typed `NotFound { name }` lets `mv: cannot stat 'X': No such
/// file or directory` work.
#[test]
fn test_rename_missing_source_returns_not_found() {
    let mut fs = fresh_fs();
    let err = fs.rename("/does-not-exist", "/anything").unwrap_err();
    assert!(
        matches!(err, Ext4Error::NotFound { ref name } if name == b"does-not-exist"),
        "expected NotFound for missing source, got {err:?}"
    );
}

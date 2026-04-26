//! Integration test: open a real `mkfs.ext4`-produced image and
//! walk it. Validates that everything we've built isn't circular —
//! the unit tests work against our own constructed images, so a
//! formatter bug paired with a matching decoder bug would still
//! pass them. This test runs against bytes a Linux `mke2fs`
//! actually wrote.
//!
//! The fixture is committed under `tests/fixtures/`; regenerate
//! via `tests/fixtures/build_real_mkfs_fixture.sh` (requires WSL2
//! or a Linux host with `e2fsprogs` on the PATH).

use std::path::PathBuf;

use ext4::{Filesystem, ROOT_INODE};
use spec::{DirEntryFileType, InodeFileType};

/// Path to the fixture, relative to the crate manifest dir.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/real_minimal.ext4")
}

/// Open the real-mkfs.ext4 fixture and walk its root.
///
/// Bug it catches: any field-offset error in our decoders that
/// happens to be symmetric with our encoders would pass the
/// per-crate unit tests (which round-trip our own output) but
/// fail here, where the bytes were written by an independent
/// implementation. Specifically catches: superblock field
/// alignment drift, GDT 32-byte-vs-64-byte handling on
/// non-64BIT images (mkfs picks 32 by default; we'd see 64-byte
/// reads if our `group_descriptor_size` rule is wrong),
/// directory-entry walking on a real lost+found dir.
#[test]
fn test_open_real_mkfs_fixture_walks_root_directory() {
    let path = fixture_path();
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open fixture {path:?}: {e}"));
    let mut fs = Filesystem::open(file).expect("Filesystem::open on real fixture");

    let sb = fs.superblock();
    assert_eq!(sb.block_size, 4096, "fixture is 4 KiB blocks");
    assert_eq!(sb.blocks_count, 128, "fixture is 128 blocks total");
    assert_eq!(sb.inodes_count, 32, "fixture has 32 inodes");
    assert_eq!(sb.volume_label(), "test", "label was set to 'test'");
    assert!(
        !sb.is_64bit(),
        "fixture was generated with 64bit feature off"
    );

    // Root dir should contain ., .., lost+found.
    let root = fs.read_inode(ROOT_INODE).expect("read root inode");
    assert!(root.is_directory(), "root must be a directory");
    assert_eq!(root.file_type(), InodeFileType::Directory);

    let entries = fs.read_dir(&root).expect("read_dir root");
    let names: Vec<&[u8]> = entries
        .iter()
        .filter(|e| !e.is_unused())
        .map(|e| e.name.as_slice())
        .collect();

    assert!(
        names.iter().any(|n| *n == b"."),
        "root should have '.' entry, got {names:?}"
    );
    assert!(
        names.iter().any(|n| *n == b".."),
        "root should have '..' entry"
    );
    assert!(
        names.iter().any(|n| *n == b"lost+found"),
        "mke2fs creates lost+found by default"
    );
}

/// Walk into `/lost+found` via path resolution and confirm it's
/// the empty-but-valid directory `mke2fs` creates.
///
/// Bug it catches: open_path on a fresh real-mkfs image is the
/// canonical user-facing operation; if it fails here, every
/// downstream consumer (vmisolate's rootfs assembly, justoci's
/// vm_image kind) is broken on the very first thing they'd try
/// to do with our reader. Also catches: extent-tree walking on
/// a directory that happens to be allocated by mkfs at a non-
/// zero physical offset (lost+found is not at block 0).
#[test]
fn test_open_path_into_lost_found_returns_directory_inode() {
    let path = fixture_path();
    let file = std::fs::File::open(&path).unwrap();
    let mut fs = Filesystem::open(file).unwrap();

    let lost_found_inode_num = fs.open_path("/lost+found").expect("/lost+found resolves");
    assert!(
        lost_found_inode_num >= 11,
        "lost+found uses a non-reserved inode (>= 11), got {lost_found_inode_num}"
    );

    let inode = fs.read_inode(lost_found_inode_num).unwrap();
    assert!(inode.is_directory(), "lost+found must be a directory");

    let entries = fs.read_dir(&inode).unwrap();
    let names: Vec<&[u8]> = entries
        .iter()
        .filter(|e| !e.is_unused())
        .map(|e| e.name.as_slice())
        .collect();
    // mke2fs's lost+found contains . and .. only (it pre-
    // allocates extra dirent slots which surface as inode=0
    // tombstones; those are filtered above).
    assert!(names.iter().any(|n| *n == b"."));
    assert!(names.iter().any(|n| *n == b".."));
}

/// Spot-check that `lost+found`'s `..` entry points back at the
/// root inode — proves directory parent links round-trip
/// correctly through both directions of our walker.
///
/// Bug it catches: a parser that reverses inode bytes in
/// dir entries (treating u32 as big-endian, say) would have
/// `..` resolve to a garbage number. The bidirectional check
/// (root → lost+found via name; lost+found → root via `..`)
/// closes the loop on directory consistency.
#[test]
fn test_lost_found_dotdot_points_back_to_root_inode() {
    let path = fixture_path();
    let file = std::fs::File::open(&path).unwrap();
    let mut fs = Filesystem::open(file).unwrap();

    let lost_found_num = fs.open_path("/lost+found").unwrap();
    let lost_found = fs.read_inode(lost_found_num).unwrap();
    let entries = fs.read_dir(&lost_found).unwrap();

    let dotdot = entries
        .iter()
        .find(|e| e.name == b"..")
        .expect("lost+found must have ..");
    assert_eq!(
        dotdot.inode, ROOT_INODE,
        "lost+found/.. should point at the root inode"
    );
    assert_eq!(dotdot.file_type(), DirEntryFileType::Directory);
}

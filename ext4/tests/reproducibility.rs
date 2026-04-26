//! Byte-stability contract for `format()` and the mkdir/create_file
//! write path. The pinned constants in `mkfs.rs` (`PINNED_TIME`,
//! `PINNED_UUID`, `PINNED_HASH_SEED`) make `format()` output
//! deterministic under the same `Config`; this file pins that as
//! a project guarantee so a future "let's read the clock here"
//! change is caught by CI rather than at the next reproducible-
//! build audit.

use std::io::Cursor;
use std::path::PathBuf;

use ext4::{format, Config, Filesystem};

/// Test fixture: a unique tempdir per test, cleaned on drop.
/// Composed off `std::env::temp_dir()` to avoid the `tempfile`
/// dep — same pattern used in `cli/src/lib.rs`.
struct Tempdir {
    path: PathBuf,
}

impl Tempdir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "justext4-repro-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Tempdir { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Tempdir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Two separate `format()` calls with the same `Config` produce
/// byte-identical images.
///
/// Bug it catches: any future change in `mkfs.rs` that pulls a
/// non-pinned value into the on-disk bytes (e.g. replacing
/// `PINNED_TIME` with `SystemTime::now()`, or seeding the UUID
/// from the OS RNG, or letting an uninitialised padding byte
/// leak through `encode_into`) breaks the reproducible-build
/// contract. Without this test such a regression is invisible
/// until two CI runs of the same commit produce different
/// artifact hashes — by then it's already shipped.
#[test]
fn test_format_with_same_config_produces_byte_identical_output() {
    let config = Config::default();

    let mut buf_a: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf_a), &config).unwrap();

    let mut buf_b: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf_b), &config).unwrap();

    // Sanity: both writes actually produced an image-sized buffer.
    // Without this, two empty Vecs would compare equal and the
    // test would pass for the wrong reason.
    let expected_len = (config.size_blocks as usize) * (config.block_size as usize);
    assert_eq!(
        buf_a.len(),
        expected_len,
        "buf_a length should equal size_blocks * block_size"
    );
    assert_eq!(buf_b.len(), expected_len, "buf_b length should match");

    assert_eq!(
        buf_a, buf_b,
        "format() output must be byte-stable across runs with the same Config"
    );
}

/// Changing only `volume_label` flips bytes inside the image.
///
/// Bug it catches: the byte-identical assertion in test 1 is
/// only meaningful if `format()` actually serialises the input
/// `Config` into the output bytes. If a refactor accidentally
/// dropped the volume-label write (or hard-coded it), test 1
/// would still pass — both calls would produce the same broken
/// image. This negative test pins that the `Config` actually
/// drives the bytes, so test 1's equality is non-trivial.
#[test]
fn test_format_with_different_labels_produces_different_output() {
    let config_a = Config {
        volume_label: b"alpha-fs".to_vec(),
        ..Config::default()
    };
    let config_b = Config {
        volume_label: b"beta-fs".to_vec(),
        ..Config::default()
    };

    let mut buf_a: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf_a), &config_a).unwrap();

    let mut buf_b: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf_b), &config_b).unwrap();

    assert_ne!(
        buf_a, buf_b,
        "different volume labels must produce different image bytes"
    );
}

/// Two host-dir fixtures with the same files, replayed into two
/// freshly-formatted images via `mkdir` + `create_file` in the
/// same order, produce byte-identical images.
///
/// Bug it catches: an allocator or directory-block layout that
/// depends on uninitialised memory, hash-map iteration order, or
/// the host filesystem's `read_dir` ordering would make the
/// build-from-tree path non-reproducible even though `format()`
/// alone is. The pinned constants only cover the empty-image
/// case; this test pins the contract for the actual user gesture
/// (build an image from a directory of files). Without it, a
/// `HashSet`-based free-block scan or a timestamp written into
/// a child inode would silently break artifact hashes.
#[test]
fn test_build_from_tree_with_same_input_produces_byte_identical_output() {
    // Two independent tempdirs with byte-identical contents and
    // the same filenames. We don't actually walk them with
    // std::fs::read_dir (whose ordering is platform-dependent on
    // Windows) — instead we replay a fixed sequence of mkdir +
    // create_file calls on each image. The tempdirs anchor the
    // "same input" framing; the deterministic replay is what
    // proves byte-stability of the write path.
    let dir_a = Tempdir::new("tree-a");
    let dir_b = Tempdir::new("tree-b");
    std::fs::create_dir_all(dir_a.join("etc")).unwrap();
    std::fs::create_dir_all(dir_b.join("etc")).unwrap();
    std::fs::write(dir_a.join("etc/hostname"), b"my-host").unwrap();
    std::fs::write(dir_b.join("etc/hostname"), b"my-host").unwrap();
    std::fs::write(dir_a.join("readme"), b"top-level file").unwrap();
    std::fs::write(dir_b.join("readme"), b"top-level file").unwrap();

    // Default config's 16 blocks doesn't leave room for nested
    // dirs + multiple files. Bump to a roomier-but-still-tiny
    // image; matters only that both runs use the exact same
    // Config so the comparison is fair.
    let config = Config {
        size_blocks: 256,
        inodes_per_group: 64,
        ..Config::default()
    };

    let buf_a = build_image(&config, &dir_a);
    let buf_b = build_image(&config, &dir_b);

    // Sanity: the build actually wrote the files we asked for.
    // Without this, an empty image vs. empty image would compare
    // equal and the assertion would pass trivially.
    let mut fs = Filesystem::open(Cursor::new(&buf_a)).unwrap();
    let hostname_inode_num = fs.open_path("/etc/hostname").unwrap();
    let inode = fs.read_inode(hostname_inode_num).unwrap();
    assert_eq!(fs.read_file(&inode).unwrap(), b"my-host");

    assert_eq!(
        buf_a, buf_b,
        "build-from-tree output must be byte-stable across runs with the same input"
    );
}

/// Helper: format a fresh image into a Vec, then replay a fixed
/// sequence of mkdir + create_file calls. The `_anchor` tempdir
/// argument exists to tie this run to a specific host-dir fixture
/// — its contents aren't read here; the deterministic replay is
/// what we're testing.
fn build_image(config: &Config, _anchor: &Tempdir) -> Vec<u8> {
    let mut buf: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf), config).unwrap();
    let mut fs = Filesystem::open(Cursor::new(&mut buf)).unwrap();
    fs.mkdir("/etc").unwrap();
    fs.create_file("/etc/hostname", b"my-host").unwrap();
    fs.create_file("/readme", b"top-level file").unwrap();
    drop(fs);
    buf
}

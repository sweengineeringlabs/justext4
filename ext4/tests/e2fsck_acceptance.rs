//! Integration test: produce an image with `mkfs::format`, run
//! `e2fsck -nf` against it, assert the kernel-grade fsck accepts
//! it without errors.
//!
//! **Always-run, skip-pass mode.** The test runs as part of
//! `cargo test` (no `#[ignore]`). When `e2fsck` is unavailable
//! on the host, it prints `SKIP` to stderr and passes — same
//! pattern justoci uses for its cosign-on-PATH probe. This
//! keeps the load-bearing kernel-grade-output check in the
//! default suite so regressions in `format()` get caught
//! anywhere e2fsck *is* available, while still letting the
//! test pass on a Windows dev box without WSL or a Linux box
//! without e2fsprogs.
//!
//! Detection logic, in order:
//!   1. `which e2fsck` succeeds → run it directly.
//!   2. `JUSTEXT4_E2FSCK_VIA_WSL=1` env var is set → run via
//!      `wsl -- e2fsck`, translating the Windows tempdir path
//!      to `/mnt/c/...`.
//!   3. Otherwise → print SKIP and pass.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ext4::{format, Config, Filesystem};

/// Build a fresh image into a tempfile and return its host path.
fn write_fresh_image() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("image.ext4");

    let mut buf: Vec<u8> = Vec::new();
    format(&mut Cursor::new(&mut buf), &Config::default()).unwrap();
    std::fs::write(&path, &buf).unwrap();
    path
}

/// Translate a Windows path to its WSL `/mnt/c/...` equivalent
/// when running under WSL wrapping. Naive but sufficient — we
/// only ever pass the tempdir path produced by `write_fresh_image`.
fn to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("C:\\").or_else(|| s.strip_prefix("C:/")) {
        return format!("/mnt/c/{}", rest.replace('\\', "/"));
    }
    s.into_owned()
}

/// What we found when we tried to invoke e2fsck.
enum E2fsckRunner {
    /// Direct invocation — `e2fsck` is on PATH.
    Direct,
    /// Wrapped: `wsl -- e2fsck`, with the image path translated
    /// to `/mnt/c/...`.
    Wsl,
    /// Couldn't find a way to invoke e2fsck on this host.
    Unavailable,
}

fn detect_runner() -> E2fsckRunner {
    // Prefer direct invocation when available.
    if Command::new("e2fsck")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
    {
        return E2fsckRunner::Direct;
    }
    // Fall back to WSL only when explicitly opted into. We
    // don't auto-spawn WSL because invoking it on a fresh
    // Windows install can be slow + interactive, which would
    // make the default test run flaky.
    if std::env::var("JUSTEXT4_E2FSCK_VIA_WSL").as_deref() == Ok("1") {
        return E2fsckRunner::Wsl;
    }
    E2fsckRunner::Unavailable
}

fn build_command(runner: &E2fsckRunner, img: &Path) -> Command {
    match runner {
        E2fsckRunner::Direct => {
            let mut c = Command::new("e2fsck");
            c.arg("-nf").arg(img);
            c
        }
        E2fsckRunner::Wsl => {
            let mut c = Command::new("wsl");
            c.arg("--").arg("e2fsck").arg("-nf").arg(to_wsl_path(img));
            c
        }
        E2fsckRunner::Unavailable => unreachable!("caller checks for Unavailable"),
    }
}

/// `e2fsck -nf` on a fresh `format()` image returns exit code 0
/// with no errors or warnings. Proves the kernel-grade fsck
/// accepts our output as a valid ext4 filesystem.
///
/// Bug it catches: any drift in our superblock encoder away
/// from the kernel's expectations (missing fields, wrong
/// `s_state`, mismatched `s_clusters_per_group`, bitmap
/// padding errors, free-count discrepancies) surfaces here as
/// an e2fsck warning. The unit tests can't catch these because
/// they only round-trip through our own decoder.
///
/// Skip-pass mode: prints `SKIP` and returns Ok when e2fsck
/// isn't reachable on the host. This keeps the test in the
/// default suite without making `cargo test` fail on a
/// developer box that doesn't have e2fsprogs installed.
#[test]
fn test_format_output_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate kernel-grade format() output here"
        );
        return;
    }

    let img = write_fresh_image();
    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    // Cleanup before assertions so a panicking test doesn't leak.
    let _ = std::fs::remove_dir_all(img.parent().unwrap());

    assert!(
        output.status.success(),
        "e2fsck rejected the image (exit code: {:?})",
        output.status.code()
    );
    // e2fsck reports "WARNING" only when it found fixable
    // inconsistencies. Reject any such warning.
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck reported warnings on our output, expected clean"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix"
    );
}

/// Format an image, write a file into it via `create_file`,
/// run `e2fsck -nf` against the result. Proves the write path
/// (allocator + inode + extent + dir entry) doesn't break the
/// kernel's view of the filesystem.
///
/// Bug it catches: the empty-format e2fsck test doesn't exercise
/// any of the allocator path. A bug in `allocate_inode` /
/// `allocate_blocks_contiguous` / `add_dir_entry` that produces
/// inconsistent bitmap accounting (e.g. "free counts wrong") or
/// dangling dir entries would only surface after a real write.
/// This test invokes the full create_file chain and validates
/// the on-disk state is still a valid ext4 filesystem from the
/// kernel's perspective.
///
/// The kernel-mount half of the proof is left as a manual
/// verification step (`mount -o loop`) because it requires sudo,
/// which we don't want to invoke from `cargo test`. The e2fsck
/// pass is a strong proxy: if e2fsck accepts the post-write
/// state as clean, the kernel will mount it (the kernel's checks
/// are a strict subset).
#[test]
fn test_format_then_create_file_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-write image state"
        );
        return;
    }

    // Tempfile path setup, identical pattern to write_fresh_image.
    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-cf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    // Format directly to disk so the create_file step opens
    // exactly what e2fsck will validate.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    // Re-open, write a file, drop the handle so the OS flushes.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    let payload = b"hello from create_file";
    fs.create_file("/hello.txt", payload).unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-create_file image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after create_file; bitmap or counter drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after create_file"
    );
}

/// `format → mkdir → create_file inside subdir → e2fsck-clean`.
/// Validates the mkdir path against kernel-grade fsck, including
/// the parent's links_count update and nested dir traversal.
///
/// Bug it catches: a mkdir that forgets to bump the parent's
/// links_count makes e2fsck flag a "Reference count wrong" in
/// pass 4. A mkdir that miscounts used_dirs_count in the GDT
/// fails pass 5 group-summary validation. A mkdir that produces
/// a directory whose `.` and `..` entries point at the wrong
/// inodes fails pass 2 directory structure check.
#[test]
fn test_format_then_mkdir_then_nested_create_file_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-mkdir image state"
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-mkdir-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    fs.mkdir("/etc").unwrap();
    fs.create_file("/etc/hostname", b"justext4\n").unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-mkdir image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after mkdir; links_count or used_dirs_count drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after mkdir"
    );
}

/// `format → create_file → unlink → e2fsck-clean`. Validates
/// the unlink path against kernel-grade fsck.
///
/// Bug it catches: an unlink that fails to clear the inode
/// bitmap (or block bitmap) leaks resources; e2fsck flags as
/// "Free blocks count wrong" or "Free inodes count wrong" in
/// pass 5. An unlink that leaves a tombstoned inode without
/// dtime set fails pass 1 inode-state validation. An unlink
/// that miscounts the parent dir's links fails pass 4.
#[test]
fn test_format_then_create_file_then_unlink_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-unlink image state"
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-unlink-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    fs.create_file("/scratch.txt", b"about to be deleted")
        .unwrap();
    fs.unlink("/scratch.txt").unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-unlink image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after unlink; bitmap drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after unlink"
    );
}

/// `format → mkdir → rmdir → e2fsck-clean`. Validates rmdir's
/// reverse-mkdir bookkeeping against kernel-grade fsck.
///
/// Bug it catches: an rmdir that doesn't decrement the
/// parent's links_count makes e2fsck flag a "Reference count
/// wrong" in pass 4. An rmdir that doesn't decrement the
/// group's used_dirs_count fails pass 5. An rmdir that frees
/// the inode bitmap bit but leaves the dir entry in place
/// fails pass 2.
#[test]
fn test_format_then_mkdir_then_rmdir_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-rmdir image state"
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-rmdir-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    fs.mkdir("/sub").unwrap();
    fs.rmdir("/sub").unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-rmdir image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after rmdir; links_count or used_dirs_count drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after rmdir"
    );
}

/// `format → symlink → e2fsck-clean`. Validates the fast-symlink
/// path against kernel-grade fsck.
///
/// Bug it catches: a symlink writer that sets `INODE_FLAG_EXTENTS`
/// on a fast symlink (target inline in i_block) makes e2fsck try
/// to parse the target string as an extent header — pass 1 fails
/// with "Inode N has EXTENTS_FL flag set on filesystem without
/// extents support" or a bad-extent-header complaint. A writer
/// that allocates a data block but doesn't track it (or vice
/// versa) drifts the bitmap free counts; e2fsck flags in pass 5.
/// A writer that uses the wrong dir-entry file_type byte (e.g. 1
/// for regular instead of 7 for symlink) triggers pass 2's
/// "Symlink ... has invalid mode/file type" check.
#[test]
fn test_format_then_symlink_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-symlink image state"
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-symlink-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    fs.symlink("/link", b"/target").unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-symlink image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after symlink; inode mode, flags, or dir-entry file_type drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after symlink"
    );
}

/// `format → create_file → truncate → e2fsck-clean`. Validates
/// the truncate path against kernel-grade fsck.
///
/// The scenario covers the load-bearing shrink case: a 2-block
/// file is truncated mid-first-block, freeing the second block
/// and rewriting the leaf extent's `len` to 1. Bug it catches:
/// a truncate that frees blocks but doesn't update the leaf
/// extent's `len` leaves a phantom range in the extent tree
/// pointing at now-free blocks; e2fsck flags it in pass 1
/// ("Inode N, i_blocks is X, should be Y" or "Inode N has
/// illegal block(s)"). A truncate that updates the extent but
/// doesn't decrement the bitmap free count drifts pass 5
/// ("Free blocks count wrong"). A truncate that forgets to
/// update `inode.size` or `blocks_lo` lands as pass 1's
/// "i_blocks is X, should be Y".
#[test]
fn test_format_then_create_file_then_truncate_passes_e2fsck_clean() {
    let runner = detect_runner();
    if matches!(runner, E2fsckRunner::Unavailable) {
        eprintln!(
            "SKIP: e2fsck not on PATH and JUSTEXT4_E2FSCK_VIA_WSL not set; \
             cannot validate post-truncate image state"
        );
        return;
    }

    let dir = std::env::temp_dir().join(format!(
        "justext4-e2fsck-truncate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let img = dir.join("image.ext4");

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&img)
        .unwrap();
    format(&mut file, &Config::default()).unwrap();
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&img)
        .unwrap();
    let mut fs = Filesystem::open(file).unwrap();
    let block_size = fs.superblock().block_size as usize;
    // Two-block payload so the shrink frees a real block.
    let payload = vec![0xAB; block_size + 1];
    fs.create_file("/shrinkme.bin", &payload).unwrap();
    // Cut to mid-first-block — exercises the partial-shrink path:
    // last extent's len drops from 2 to 1, the second physical
    // block is freed, inode.size + blocks_lo are rewritten.
    fs.truncate("/shrinkme.bin", (block_size / 2) as u64)
        .unwrap();
    drop(fs);

    let mut cmd = build_command(&runner, &img);
    eprintln!("running: {cmd:?}");
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("--- e2fsck stdout ---\n{stdout}");
    eprintln!("--- e2fsck stderr ---\n{stderr}");

    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "e2fsck rejected post-truncate image (exit code: {:?})",
        output.status.code()
    );
    assert!(
        !stdout.contains("WARNING"),
        "e2fsck flagged warnings after truncate; bitmap/extent/blocks_lo drift?"
    );
    assert!(
        !stdout.contains("Fix?"),
        "e2fsck found a problem requiring fix after truncate"
    );
}

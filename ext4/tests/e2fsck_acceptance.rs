//! Integration test: produce an image with `mkfs::format`, run
//! `e2fsck -nf` against it, assert the kernel-grade fsck accepts
//! it without errors.
//!
//! Gated `#[ignore]` because it requires `e2fsck` on the host's
//! PATH (typically via Linux or WSL on Windows). Run with:
//!
//! ```bash
//! cargo test -p swe_justext4_ext4 --test e2fsck_acceptance \
//!     -- --ignored --nocapture
//! ```
//!
//! On a Windows dev box, set `JUSTEXT4_E2FSCK_VIA_WSL=1` to wrap
//! the call in `wsl -- bash -c`, translating the Windows tempdir
//! path into `/mnt/c/...`. The test prints what it executed so
//! a failure reads cleanly.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ext4::{format, Config};

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
#[test]
#[ignore]
fn test_format_output_passes_e2fsck_clean() {
    let img = write_fresh_image();
    let via_wsl = std::env::var("JUSTEXT4_E2FSCK_VIA_WSL").as_deref() == Ok("1");

    let mut cmd = if via_wsl {
        let mut c = Command::new("wsl");
        c.arg("--").arg("e2fsck").arg("-nf").arg(to_wsl_path(&img));
        c
    } else {
        let mut c = Command::new("e2fsck");
        c.arg("-nf").arg(&img);
        c
    };
    println!("running: {cmd:?}");

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn e2fsck");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("--- e2fsck stdout ---\n{stdout}");
    println!("--- e2fsck stderr ---\n{stderr}");

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

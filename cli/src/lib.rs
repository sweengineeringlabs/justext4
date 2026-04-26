//! `mkext4-rs` library half — argument parsers and command
//! implementations exposed as functions so the binary can stay
//! tiny and integration tests can call commands without
//! shelling out.
//!
//! The `main.rs` is a thin shim: dispatch on `argv[1]`, wire
//! stdout/stderr/exit-code, delegate to one of the `cmd_*`
//! functions here.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use ext4::{format as fs_format, Config, Filesystem};

/// Top-level error type — prefer plain `String` over `thiserror`
/// here because the CLI just renders the message and exits.
/// Lower layers already produce richly-typed errors; this is
/// the boundary that converts them to user-facing strings.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

impl<E: std::fmt::Display> From<(&str, E)> for CliError {
    fn from((ctx, err): (&str, E)) -> Self {
        CliError(format!("{ctx}: {err}"))
    }
}

/// Print top-level usage to the supplied writer (so tests can
/// capture and assert on it).
pub fn print_usage<W: Write>(out: &mut W) -> std::io::Result<()> {
    writeln!(out, "mkext4-rs — pure-Rust ext4 image tool")?;
    writeln!(out)?;
    writeln!(out, "Usage:")?;
    writeln!(
        out,
        "  mkext4-rs format <path> [--size-blocks N] [--block-size N] [--label TEXT]"
    )?;
    writeln!(out, "  mkext4-rs inspect <path>")?;
    writeln!(out, "  mkext4-rs touch <image> <vfs-path> <content>")?;
    writeln!(out, "  mkext4-rs cat <image> <vfs-path>")?;
    writeln!(out, "  mkext4-rs chmod <image> <vfs-path> <octal-mode>")?;
    writeln!(out, "  mkext4-rs chown <image> <vfs-path> <uid> <gid>")?;
    writeln!(
        out,
        "  mkext4-rs utime <image> <vfs-path> <atime-epoch> <mtime-epoch>"
    )?;
    writeln!(
        out,
        "  mkext4-rs build-from-tree <host-dir> <image> [--size-blocks N] [--inodes N] [--label TEXT]"
    )?;
    Ok(())
}

/// Format a fresh ext4 image at the given filesystem path.
pub fn cmd_format<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    let host_path = args
        .first()
        .ok_or_else(|| CliError("format requires a host path argument".to_string()))?;

    let mut config = Config::default();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--size-blocks" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--size-blocks requires a value".to_string()))?;
                config.size_blocks = value
                    .parse::<u32>()
                    .map_err(|e| CliError(format!("--size-blocks parse: {e}")))?;
                i += 2;
            }
            "--block-size" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--block-size requires a value".to_string()))?;
                config.block_size = value
                    .parse::<u32>()
                    .map_err(|e| CliError(format!("--block-size parse: {e}")))?;
                i += 2;
            }
            "--label" => {
                let value = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--label requires a value".to_string()))?;
                config.volume_label = value.as_bytes().to_vec();
                i += 2;
            }
            other => {
                return Err(CliError(format!("unknown flag: {other}")));
            }
        }
    }

    let mut file = OpenOptions::new()
        .write(true)
        .read(true)
        .create(true)
        .truncate(true)
        .open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    fs_format(&mut file, &config).map_err(|e| CliError(format!("format: {e}")))?;

    let total_bytes = (config.size_blocks as u64) * (config.block_size as u64);
    writeln!(
        out,
        "formatted {host_path}: {} blocks of {} bytes ({} bytes total)",
        config.size_blocks, config.block_size, total_bytes,
    )
    .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Dump the superblock + root directory listing for a host-path
/// ext4 image.
pub fn cmd_inspect<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    let host_path = args
        .first()
        .ok_or_else(|| CliError("inspect requires a host path argument".to_string()))?;

    let file = std::fs::File::open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;

    let sb = fs.superblock();
    writeln!(out, "{host_path}:").map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  block_size:       {}", sb.block_size)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  blocks_count:     {}", sb.blocks_count)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  free_blocks:      {}", sb.free_blocks_count)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  inodes_count:     {}", sb.inodes_count)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  free_inodes:      {}", sb.free_inodes_count)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  blocks_per_group: {}", sb.blocks_per_group)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  inodes_per_group: {}", sb.inodes_per_group)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  inode_size:       {}", sb.inode_size)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  rev_level:        {}", sb.rev_level)
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  is_64bit:         {}", sb.is_64bit())
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out, "  volume_label:     {:?}", sb.volume_label())
        .map_err(|e| CliError(format!("write: {e}")))?;
    writeln!(out).map_err(|e| CliError(format!("write: {e}")))?;

    let root_inode_num = ext4::ROOT_INODE;
    let root = fs
        .read_inode(root_inode_num)
        .map_err(|e| CliError(format!("read root: {e}")))?;
    let entries = fs
        .read_dir(&root)
        .map_err(|e| CliError(format!("read_dir /: {e}")))?;
    writeln!(
        out,
        "/ ({} entries):",
        entries.iter().filter(|e| !e.is_unused()).count()
    )
    .map_err(|e| CliError(format!("write: {e}")))?;
    for entry in entries {
        if entry.is_unused() {
            continue;
        }
        writeln!(
            out,
            "  inode={:<5} type={} {:?}",
            entry.inode,
            entry.file_type_raw,
            entry.name_str_lossy()
        )
        .map_err(|e| CliError(format!("write: {e}")))?;
    }
    Ok(())
}

/// Create a regular file inside the image at the given vfs path
/// with the given contents.
pub fn cmd_touch<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 3 {
        return Err(CliError(
            "touch requires <image> <vfs-path> <content>".to_string(),
        ));
    }
    let host_path = &args[0];
    let vfs_path = &args[1];
    let content = args[2].as_bytes();

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    let inode_num = fs
        .create_file(vfs_path, content)
        .map_err(|e| CliError(format!("create_file: {e}")))?;
    writeln!(
        out,
        "created {vfs_path} (inode {inode_num}, {} bytes)",
        content.len()
    )
    .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Walk a host directory tree and replicate it inside the
/// image. Subdirectories become ext4 dirs (`mkdir`); regular
/// files become ext4 regular files with their content copied
/// (`create_file`); symlinks become ext4 symlinks (`symlink`)
/// when their target fits v0's fast-symlink inline limit (60
/// bytes); other file types (devices, sockets, fifos) are
/// skipped with a warning.
///
/// Walks breadth-first via a stack so deeply nested trees don't
/// blow the host stack — recursive `read_dir` would hit Rust's
/// default 8 MB limit on a deep `node_modules`-shaped tree.
pub fn populate_from_host_tree<R: std::io::Read + Write + std::io::Seek>(
    fs: &mut ext4::Filesystem<R>,
    host_root: &Path,
) -> Result<(), CliError> {
    let host_root = host_root
        .canonicalize()
        .map_err(|e| CliError(format!("canonicalize {host_root:?}: {e}")))?;
    let metadata =
        std::fs::metadata(&host_root).map_err(|e| CliError(format!("stat {host_root:?}: {e}")))?;
    if !metadata.is_dir() {
        return Err(CliError(format!("{host_root:?} is not a directory")));
    }

    // Stack of (host path, vfs path-in-image) pairs to process.
    let mut stack: Vec<(std::path::PathBuf, String)> = vec![(host_root.clone(), String::new())];

    while let Some((host_dir, vfs_dir)) = stack.pop() {
        let read_dir = std::fs::read_dir(&host_dir)
            .map_err(|e| CliError(format!("read_dir {host_dir:?}: {e}")))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| CliError(format!("dir entry in {host_dir:?}: {e}")))?;
            let name = entry.file_name();
            let name_str = match name.to_str() {
                Some(s) => s,
                None => {
                    eprintln!("warning: skipping non-UTF-8 name in {host_dir:?}");
                    continue;
                }
            };
            let vfs_child = format!("{vfs_dir}/{name_str}");
            let file_type = entry
                .file_type()
                .map_err(|e| CliError(format!("file_type {name:?}: {e}")))?;
            if file_type.is_dir() {
                fs.mkdir(&vfs_child)
                    .map_err(|e| CliError(format!("mkdir {vfs_child}: {e}")))?;
                stack.push((entry.path(), vfs_child));
            } else if file_type.is_file() {
                let bytes = std::fs::read(entry.path())
                    .map_err(|e| CliError(format!("read {:?}: {e}", entry.path())))?;
                fs.create_file(&vfs_child, &bytes)
                    .map_err(|e| CliError(format!("create_file {vfs_child}: {e}")))?;
            } else if file_type.is_symlink() {
                // Read the symlink's target and replicate it as
                // an ext4 fast symlink. Targets are arbitrary
                // bytes on Linux; on Windows `read_link` may
                // give a path that loses information when
                // converted via to_string_lossy, but for v0's
                // fast-symlink store-target-bytes-verbatim the
                // lossy conversion is the right trade — we want
                // round-tripping a Linux tree through Windows
                // not to fail on encoding.
                let target = std::fs::read_link(entry.path())
                    .map_err(|e| CliError(format!("read_link {:?}: {e}", entry.path())))?;
                let target_bytes = target.to_string_lossy().into_owned().into_bytes();
                match fs.symlink(&vfs_child, &target_bytes) {
                    Ok(_) => {}
                    Err(ext4::Ext4Error::SymlinkTargetTooLong { len }) => {
                        eprintln!(
                            "warning: skipping symlink {vfs_child}: target is {len} bytes; \
                             v0 supports fast symlinks only (target <= 60 bytes)"
                        );
                    }
                    Err(e) => {
                        return Err(CliError(format!("symlink {vfs_child}: {e}")));
                    }
                }
            } else {
                eprintln!(
                    "warning: skipping {vfs_child}: file type {file_type:?} not supported in v0"
                );
            }
        }
    }
    Ok(())
}

/// Format a fresh ext4 image and populate it from a host
/// directory tree. The single user-facing gesture for "turn this
/// dir into a kernel-mountable rootfs."
pub fn cmd_build_from_tree<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 2 {
        return Err(CliError(
            "build-from-tree requires <host-dir> <image> [--size-blocks N] [--inodes N] [--label TEXT]"
                .to_string(),
        ));
    }
    let host_dir = std::path::PathBuf::from(&args[0]);
    let image_path = &args[1];
    let mut config = Config::default();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--size-blocks" => {
                config.size_blocks = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--size-blocks requires a value".to_string()))?
                    .parse::<u32>()
                    .map_err(|e| CliError(format!("--size-blocks parse: {e}")))?;
                i += 2;
            }
            "--inodes" => {
                config.inodes_per_group = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--inodes requires a value".to_string()))?
                    .parse::<u32>()
                    .map_err(|e| CliError(format!("--inodes parse: {e}")))?;
                i += 2;
            }
            "--label" => {
                config.volume_label = args
                    .get(i + 1)
                    .ok_or_else(|| CliError("--label requires a value".to_string()))?
                    .as_bytes()
                    .to_vec();
                i += 2;
            }
            other => return Err(CliError(format!("unknown flag: {other}"))),
        }
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(Path::new(image_path))
        .map_err(|e| CliError(format!("open {image_path}: {e}")))?;
    fs_format(&mut file, &config).map_err(|e| CliError(format!("format: {e}")))?;
    drop(file);

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(image_path))
        .map_err(|e| CliError(format!("re-open {image_path}: {e}")))?;
    let mut fs = ext4::Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    populate_from_host_tree(&mut fs, &host_dir)?;

    writeln!(
        out,
        "built {image_path} from {host_dir:?}: {} blocks of {} bytes, {} inode slots",
        config.size_blocks, config.block_size, config.inodes_per_group,
    )
    .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Mutate inode mode bits (POSIX `chmod`). Replaces the bottom
/// 12 bits; preserves the file-type nibble.
///
/// Args: `<image> <vfs-path> <octal-mode>` — the mode is parsed
/// as octal (`0755`, `755`, both work) per the unix tradition.
pub fn cmd_chmod<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 3 {
        return Err(CliError(
            "chmod requires <image> <vfs-path> <octal-mode>".to_string(),
        ));
    }
    let host_path = &args[0];
    let vfs_path = &args[1];
    let mode_str = args[2].trim_start_matches('0');
    let mode_str = if mode_str.is_empty() { "0" } else { mode_str };
    let mode = u16::from_str_radix(mode_str, 8)
        .map_err(|e| CliError(format!("chmod parse mode {:?}: {e}", args[2])))?;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    fs.chmod(vfs_path, mode)
        .map_err(|e| CliError(format!("chmod: {e}")))?;
    writeln!(out, "chmod {vfs_path} -> {:o}", mode).map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Mutate inode owner uid + gid (POSIX `chown`).
///
/// Args: `<image> <vfs-path> <uid> <gid>` — both ids are parsed
/// as decimal `u32`; values above 65535 are written through the
/// OSD2 high-word path.
pub fn cmd_chown<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 4 {
        return Err(CliError(
            "chown requires <image> <vfs-path> <uid> <gid>".to_string(),
        ));
    }
    let host_path = &args[0];
    let vfs_path = &args[1];
    let uid = args[2]
        .parse::<u32>()
        .map_err(|e| CliError(format!("chown parse uid {:?}: {e}", args[2])))?;
    let gid = args[3]
        .parse::<u32>()
        .map_err(|e| CliError(format!("chown parse gid {:?}: {e}", args[3])))?;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    fs.chown(vfs_path, uid, gid)
        .map_err(|e| CliError(format!("chown: {e}")))?;
    writeln!(out, "chown {vfs_path} -> uid={uid} gid={gid}")
        .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Mutate inode timestamps atime + mtime (POSIX `utime`). ctime
/// is bumped by the underlying op per POSIX rules — callers don't
/// supply it.
///
/// Args: `<image> <vfs-path> <atime-epoch> <mtime-epoch>` — both
/// stamps are POSIX seconds (`u32`), matching ext4's
/// resolution for v0.
pub fn cmd_utime<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 4 {
        return Err(CliError(
            "utime requires <image> <vfs-path> <atime-epoch> <mtime-epoch>".to_string(),
        ));
    }
    let host_path = &args[0];
    let vfs_path = &args[1];
    let atime = args[2]
        .parse::<u32>()
        .map_err(|e| CliError(format!("utime parse atime {:?}: {e}", args[2])))?;
    let mtime = args[3]
        .parse::<u32>()
        .map_err(|e| CliError(format!("utime parse mtime {:?}: {e}", args[3])))?;

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    fs.utime(vfs_path, atime, mtime)
        .map_err(|e| CliError(format!("utime: {e}")))?;
    writeln!(out, "utime {vfs_path} -> atime={atime} mtime={mtime}")
        .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

/// Read the contents of a file inside the image and write them
/// verbatim to the writer.
pub fn cmd_cat<W: Write>(args: &[String], out: &mut W) -> Result<(), CliError> {
    if args.len() < 2 {
        return Err(CliError("cat requires <image> <vfs-path>".to_string()));
    }
    let host_path = &args[0];
    let vfs_path = &args[1];

    let file = std::fs::File::open(Path::new(host_path))
        .map_err(|e| CliError(format!("open {host_path}: {e}")))?;
    let mut fs = Filesystem::open(file).map_err(|e| CliError(format!("open: {e}")))?;
    let inode_num = fs
        .open_path(vfs_path)
        .map_err(|e| CliError(format!("open_path: {e}")))?;
    let inode = fs
        .read_inode(inode_num)
        .map_err(|e| CliError(format!("read_inode: {e}")))?;
    let bytes = fs
        .read_file(&inode)
        .map_err(|e| CliError(format!("read_file: {e}")))?;
    out.write_all(&bytes)
        .map_err(|e| CliError(format!("write: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Test fixture: a unique tempdir per test, cleaned on drop.
    /// We avoid the `tempfile` dep by composing one off
    /// `std::env::temp_dir()`.
    struct Tempdir {
        path: PathBuf,
    }

    impl Tempdir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "justext4-cli-{}-{}-{}",
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

    /// Format → inspect → assert the output names the expected
    /// fields. Exercises the CLI commands end-to-end against a
    /// real on-disk file.
    ///
    /// Bug it catches: the cmd_* functions are the only path
    /// users actually invoke; if format() takes a Vec but the
    /// CLI passes a File, or if inspect() can't open what
    /// format() wrote (file mode mismatch, truncation), the
    /// integration is broken even though the underlying ext4
    /// crate's tests pass.
    #[test]
    fn test_cli_format_then_inspect_round_trips() {
        let dir = Tempdir::new("fmt-inspect");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        // format
        let mut out = Vec::new();
        cmd_format(
            &[img_str.clone(), "--label".into(), "test-label".into()],
            &mut out,
        )
        .unwrap();
        let formatted_msg = String::from_utf8(out).unwrap();
        assert!(formatted_msg.contains("formatted"));

        // inspect
        let mut out = Vec::new();
        cmd_inspect(&[img_str], &mut out).unwrap();
        let inspect_msg = String::from_utf8(out).unwrap();
        assert!(
            inspect_msg.contains("test-label"),
            "inspect output should contain volume label, got: {inspect_msg}"
        );
        assert!(
            inspect_msg.contains("block_size:       4096"),
            "inspect should report block_size: {inspect_msg}"
        );
        assert!(inspect_msg.contains("/ ("));
    }

    /// Format → touch → cat returns the bytes the touch wrote.
    ///
    /// Bug it catches: end-to-end demo for the write path
    /// through the CLI. A user runs `format`, `touch`, `cat` and
    /// expects the bytes back. If touch's create_file or cat's
    /// read_file disagree about how to find the file, the user-
    /// facing flow is broken.
    #[test]
    fn test_cli_format_then_touch_then_cat_round_trips() {
        let dir = Tempdir::new("touch-cat");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[
                img_str.clone(),
                "/greeting.txt".into(),
                "hello, ext4!".into(),
            ],
            &mut sink,
        )
        .unwrap();

        let mut bytes = Vec::new();
        cmd_cat(&[img_str, "/greeting.txt".into()], &mut bytes).unwrap();
        assert_eq!(bytes, b"hello, ext4!");
    }

    /// Missing arg surfaces a typed CliError with a helpful
    /// message rather than a panic.
    #[test]
    fn test_cli_format_no_path_returns_error() {
        let mut sink = Vec::new();
        let err = cmd_format(&[], &mut sink).unwrap_err();
        assert!(err.0.contains("requires a host path"));
    }

    /// `build-from-tree` walks a small host tree and produces
    /// an image whose contents read back through our reader.
    ///
    /// Bug it catches: any path/name encoding glitch, mkdir/
    /// create_file ordering bug, or stack-vs-recursion mistake
    /// in the walker. The test fixture spans two directory
    /// levels with regular files at each level — exercises the
    /// full walker path, not just one call.
    #[test]
    fn test_cli_build_from_tree_replicates_host_tree() {
        let dir = Tempdir::new("build-tree");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        // Build the host fixture: /etc/hostname + /etc/conf.d/network
        let host = dir.join("rootfs");
        std::fs::create_dir_all(host.join("etc/conf.d")).unwrap();
        std::fs::write(host.join("etc/hostname"), b"my-host").unwrap();
        std::fs::write(host.join("etc/conf.d/network"), b"nic0=up").unwrap();
        std::fs::write(host.join("readme"), b"top-level file").unwrap();
        let host_str = host.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_build_from_tree(
            &[
                host_str,
                img_str.clone(),
                "--inodes".into(),
                "64".into(),
                "--size-blocks".into(),
                "256".into(),
            ],
            &mut sink,
        )
        .unwrap();

        // Now read the image back and verify content.
        let mut fs = ext4::Filesystem::open(std::fs::File::open(&img).unwrap()).unwrap();
        let n = fs.open_path("/etc/hostname").unwrap();
        let inode = fs.read_inode(n).unwrap();
        assert_eq!(fs.read_file(&inode).unwrap(), b"my-host");
        let n = fs.open_path("/etc/conf.d/network").unwrap();
        let inode = fs.read_inode(n).unwrap();
        assert_eq!(fs.read_file(&inode).unwrap(), b"nic0=up");
        let n = fs.open_path("/readme").unwrap();
        let inode = fs.read_inode(n).unwrap();
        assert_eq!(fs.read_file(&inode).unwrap(), b"top-level file");
    }

    /// Unknown flag surfaces a typed CliError.
    #[test]
    fn test_cli_format_unknown_flag_returns_error() {
        let dir = Tempdir::new("bad-flag");
        let img = dir.join("image.ext4");
        let mut sink = Vec::new();
        let err = cmd_format(
            &[img.to_string_lossy().into_owned(), "--bogus".into()],
            &mut sink,
        )
        .unwrap_err();
        assert!(err.0.contains("unknown flag"));
    }

    /// Format → touch → chmod → re-open and read inode → mode
    /// matches the supplied octal value with the file-type
    /// nibble preserved.
    ///
    /// Bug it catches: end-to-end CLI flow for `chmod`. Catches
    /// the same class of bug as the lib-level chmod test, but
    /// also catches CLI-side mistakes — wrong octal-parse base,
    /// wrong arg ordering, file handle not flushed before re-
    /// open, etc.
    #[test]
    fn test_cli_format_then_touch_then_chmod_round_trips() {
        let dir = Tempdir::new("chmod");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[img_str.clone(), "/file.txt".into(), "data".into()],
            &mut sink,
        )
        .unwrap();
        cmd_chmod(
            &[img_str.clone(), "/file.txt".into(), "0755".into()],
            &mut sink,
        )
        .unwrap();

        // Re-open and verify the on-disk mode.
        let file = std::fs::File::open(&img).unwrap();
        let mut fs = ext4::Filesystem::open(file).unwrap();
        let inum = fs.open_path("/file.txt").unwrap();
        let inode = fs.read_inode(inum).unwrap();
        assert_eq!(
            inode.mode, 0o100755,
            "expected S_IFREG | 0o755 = 0o100755, got {:o}",
            inode.mode
        );
    }

    /// Format → touch → chown → re-open and read inode → uid
    /// and gid match the supplied decimal values.
    #[test]
    fn test_cli_format_then_touch_then_chown_round_trips() {
        let dir = Tempdir::new("chown");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[img_str.clone(), "/file.txt".into(), "data".into()],
            &mut sink,
        )
        .unwrap();
        cmd_chown(
            &[
                img_str.clone(),
                "/file.txt".into(),
                "1000".into(),
                "1001".into(),
            ],
            &mut sink,
        )
        .unwrap();

        let file = std::fs::File::open(&img).unwrap();
        let mut fs = ext4::Filesystem::open(file).unwrap();
        let inum = fs.open_path("/file.txt").unwrap();
        let inode = fs.read_inode(inum).unwrap();
        assert_eq!(inode.uid, 1000, "uid must round-trip");
        assert_eq!(inode.gid, 1001, "gid must round-trip");
    }

    /// Format → touch → utime → re-open and read inode →
    /// atime + mtime match the supplied epoch values.
    ///
    /// Bug it catches: ordering mistake in CLI parsing (atime
    /// and mtime swapped), missing file-handle flush, or the
    /// underlying utime not bumping ctime — the assertion on
    /// ctime catches the POSIX-rule violation directly.
    #[test]
    fn test_cli_format_then_touch_then_utime_round_trips() {
        let dir = Tempdir::new("utime");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[img_str.clone(), "/file.txt".into(), "data".into()],
            &mut sink,
        )
        .unwrap();
        cmd_utime(
            &[
                img_str.clone(),
                "/file.txt".into(),
                "1700000000".into(),
                "1700001000".into(),
            ],
            &mut sink,
        )
        .unwrap();

        let file = std::fs::File::open(&img).unwrap();
        let mut fs = ext4::Filesystem::open(file).unwrap();
        let inum = fs.open_path("/file.txt").unwrap();
        let inode = fs.read_inode(inum).unwrap();
        assert_eq!(inode.atime, 1_700_000_000, "atime round-trip");
        assert_eq!(inode.mtime, 1_700_001_000, "mtime round-trip");
        assert_eq!(
            inode.ctime,
            ext4::METADATA_CTIME,
            "utime must still bump ctime per POSIX"
        );
    }

    /// `chmod` with a missing arg surfaces a typed CliError
    /// rather than a panic.
    #[test]
    fn test_cli_chmod_missing_args_returns_error() {
        let mut sink = Vec::new();
        let err = cmd_chmod(&["only-image".into()], &mut sink).unwrap_err();
        assert!(
            err.0.contains("requires"),
            "expected 'requires' in error, got: {}",
            err.0
        );
    }

    /// `chown` with a non-numeric uid surfaces a typed parse
    /// error rather than panicking.
    #[test]
    fn test_cli_chown_bad_uid_returns_error() {
        let dir = Tempdir::new("chown-bad-uid");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[img_str.clone(), "/file.txt".into(), "data".into()],
            &mut sink,
        )
        .unwrap();
        let err = cmd_chown(
            &[
                img_str,
                "/file.txt".into(),
                "not-a-number".into(),
                "0".into(),
            ],
            &mut sink,
        )
        .unwrap_err();
        assert!(
            err.0.contains("parse uid"),
            "expected 'parse uid' in error, got: {}",
            err.0
        );
    }

    /// `utime` with a non-numeric atime surfaces a typed parse
    /// error rather than panicking.
    #[test]
    fn test_cli_utime_bad_atime_returns_error() {
        let dir = Tempdir::new("utime-bad-atime");
        let img = dir.join("image.ext4");
        let img_str = img.to_string_lossy().into_owned();

        let mut sink = Vec::new();
        cmd_format(std::slice::from_ref(&img_str), &mut sink).unwrap();
        cmd_touch(
            &[img_str.clone(), "/file.txt".into(), "data".into()],
            &mut sink,
        )
        .unwrap();
        let err = cmd_utime(
            &[
                img_str,
                "/file.txt".into(),
                "not-a-number".into(),
                "0".into(),
            ],
            &mut sink,
        )
        .unwrap_err();
        assert!(
            err.0.contains("parse atime"),
            "expected 'parse atime' in error, got: {}",
            err.0
        );
    }
}

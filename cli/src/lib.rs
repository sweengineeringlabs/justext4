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
}

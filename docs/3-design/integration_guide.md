# Integration guide

How to integrate justext4 into other systems. Two integration shapes are supported: as a Rust library (the `ext4` crate), and as a CLI binary (`mkext4-rs`).

## As a library

Add the `ext4` crate as a path or git dep in your `Cargo.toml`:

```toml
[dependencies]
ext4 = { package = "swe_justext4_ext4", git = "https://github.com/sweengineeringlabs/justext4", tag = "v0.1.0" }
```

Build an image from a source tree:

```rust
use ext4::Filesystem;
use std::fs::File;
use std::io::Cursor;

fn build_image(out_path: &str) -> std::io::Result<()> {
    // 4 MB image; expand with format options if you need a different
    // block size or feature set.
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let cursor = Cursor::new(&mut buf[..]);
    let mut fs = Filesystem::format(cursor, Default::default())?;

    fs.mkdir("/etc", 0o755, 0, 0)?;
    let mut f = fs.create_file("/etc/hostname", 0o644, 0, 0)?;
    f.write_all(b"justext4\n")?;

    std::fs::write(out_path, &buf)
}
```

Read a file from an existing image:

```rust
use ext4::Filesystem;
use std::fs::File;

let img = File::open("rootfs.img")?;
let fs = Filesystem::open(img)?;
let mut node = fs.open("/etc/hostname")?;
let mut buf = String::new();
node.read_to_string(&mut buf)?;
```

The `Filesystem` opens over any `Read + Write + Seek` byte stream — `File`, `Cursor<&mut [u8]>`, an in-memory tempfile, or your own custom stream. No subprocess, no `mke2fs`, no FUSE.

See `ext4/src/lib.rs` for the full public surface (`open`, `read`, `create_file`, `mkdir`, `unlink`, `rmdir`, `symlink`, `truncate`, `rename`, `chmod`, `chown`, `utime`).

## As a CLI binary

The `mkext4-rs` binary covers the operator-side gestures without writing Rust.

```sh
# Format a fresh 4 MB image
mkext4-rs format rootfs.img --size 4M

# Build an image from a source directory tree
mkext4-rs build-from-tree ./rootfs-src/ rootfs.img --size 4M

# Inspect an existing image
mkext4-rs inspect rootfs.img

# Stat / cat / chmod / chown / utime — operator-friendly gestures
mkext4-rs cat rootfs.img /etc/hostname
mkext4-rs chmod rootfs.img /etc/hostname 0644
```

See `cli/src/main.rs` for the full subcommand list. All gestures are scriptable; exit codes follow the `0 = success / non-zero = typed-error` convention documented in the operator manual.

## Reproducibility contract

justext4's output is **byte-stable** when given the same inputs. Determinism is the load-bearing differentiator vs `mke2fs`:

- UUIDs come from a caller-supplied seed (default: zero) — not the system random.
- Timestamps come from a caller-supplied `mtime` — not `now()`.
- Allocator decisions are deterministic given the same input file order.

This is what makes justext4 usable in reproducible-image pipelines (tagging container images by content hash where two builds of the same Dockerfile must produce bit-identical bytes).

## vmisolate as a worked example

The vmisolate microVM project consumes justext4 as the rootfs builder. See `../vmisolate/main/features/rootfs/` (path-dep into justext4) for the integration. Two paths exist: `--features pure-rust` swaps the default mke2fs subprocess pipeline for justext4. `Cargo.toml` declares justext4 as an optional path-dep gated on the feature.

## Feature flags

| Crate | Flag | Purpose |
|---|---|---|
| `ext4` | (none in v0) | All functionality is on by default. |
| `cli` | (none in v0) | All subcommands are on by default. |

If your environment can't tolerate any of the std-feature deps (none currently — justext4 is `std` + RustCrypto-style minimal), file an issue describing the constraint and we'll consider a `no_std` carve-out.

## Wire compatibility

justext4-produced images are:

- **e2fsck-clean**: the kernel's user-space `e2fsck` validator passes without errors on every output of `format` / `build-from-tree`.
- **Mountable on Linux ≥ 4.x**: the kernel ext4 driver accepts the image with `mount -o loop`.
- **v0 limitations**: single block group only, read-only journal flag, no extended attributes, no quotas. See the v0-gap GitHub issue list for the unowned features.

Live integration tests (`mount -o loop` + `e2fsck`) follow the skip-pass pattern: they run by default and print `SKIP` if the host doesn't have the necessary tools. Set `JUSTEXT4_E2FSCK=1` and `JUSTEXT4_MOUNT=1` to require the live checks (intended for CI hosts).

## Stability promise

The library API surface (`ext4::Filesystem` + its `open` / `read` / write methods) is stable. Wire format follows the ext4 standard — backwards-compatible changes only. The CLI flags follow `cargo`'s convention: short flags may evolve, long flags are stable across minor versions.

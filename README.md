# justext4

**Pure-Rust ext4 read + write. No `mkfs.ext4` wrapper, no FFI.**

A standalone library for parsing and producing ext4 filesystem images
without shelling out to `e2fsprogs`. Targets the gap in the Rust
ecosystem: read-only ext4 crates exist (`ext4`, `ext4fs`,
`linux-ext4`), but **nothing on crates.io is write-capable**.

This is a primitive, not an application. Consumers:

- **VM image builders** that want to produce `rootfs.ext4` on Windows
  hosts without WSL2 (`mkfs.ext4` is Linux-only).
- **Reproducible-build pipelines** that need bit-identical output for
  a given input — `mkfs.ext4` injects non-deterministic UUIDs and
  metadata-csum seeds.
- **Embedded firmware tooling** that ships ext4 images as flashable
  artifacts.

## Status

Working v0. Round-trip demo passes — format an empty image, write a
file, read the bytes back, all in process:

```rust
use std::io::Cursor;
use ext4::{format, Config, Filesystem};

let mut img: Vec<u8> = Vec::new();
format(&mut Cursor::new(&mut img), &Config::default())?;

let mut fs = Filesystem::open(Cursor::new(&mut img))?;
fs.create_file("/hello.txt", b"hello, ext4!")?;

let inode_num = fs.open_path("/hello.txt")?;
let inode = fs.read_inode(inode_num)?;
assert_eq!(fs.read_file(&inode)?, b"hello, ext4!");
```

What's working:

- **Read path**: superblock, group descriptors, inodes, extent trees,
  directory entries, file content, path resolution.
- **Write path**: `format` / `create_file` / `mkdir` / `unlink` /
  `rmdir` / `symlink` / `truncate` / `rename` / `chmod` / `chown` /
  `utime`. Bitmap allocator. Build-from-host-tree.
- **Kernel interop, both directions**: we read real `mkfs.ext4`
  output (committed fixture under `ext4/tests/fixtures/`), and our
  output passes `e2fsck -nf` clean and is mountable by the Linux
  kernel via `mount -o loop` (verified end-to-end). The e2fsck
  test runs on every CI push (Ubuntu has e2fsprogs preinstalled).
- **Reproducibility**: same `Config` produces byte-identical output
  across runs (pinned timestamps + UUID + hash seed; pinned by an
  always-on test).
- **Fuzz harness**: `fuzz/` sub-project with five `cargo-fuzz`
  targets (one per spec-layer decoder). Run with
  `cd fuzz && cargo +nightly fuzz run <target>`.

What's not yet (each tracked as a GitHub issue):

- Long symlinks (target > 60 bytes — fast-symlink only today)
- Symlink-following in `open_path` (lstat semantics, not stat)
- Append / write-into-existing-file
- xattr (extended attributes)
- mknod / device files
- Multi-group images (single group only — caps at ~128 MiB at
  4 KiB blocks)
- Block fragmentation (contiguous-only allocator)
- Hash-tree directories (`EXT4_INDEX_FL`) — large dirs can't enumerate
- Inline-data inodes (`INLINE_DATA` feature)
- JBD2 journal, `METADATA_CSUM`, `64BIT` features
- crates.io publication

## Crates

| Crate                | Role                                                           |
|----------------------|----------------------------------------------------------------|
| `swe_justext4_spec`  | On-disk format types — superblock, group descriptor, inode, extent, dir entry, bitmap helpers. Pure structs + decode/encode. No IO. |
| `swe_justext4_ext4`  | Read + write API. `Filesystem::open` for existing images, `mkfs::format` to create new ones, `create_file` / `read_file` / `read_dir` / `open_path` for content traversal. |
| `swe_justext4_cli`   | `mkext4-rs` operator binary. Format, inspect, touch, cat. |

## CLI

```
mkext4-rs format <path> [--size-blocks N] [--block-size N] [--label TEXT]
mkext4-rs inspect <path>
mkext4-rs touch <image> <vfs-path> <content>
mkext4-rs cat <image> <vfs-path>
```

## Documentation

See [`docs/README.md`](docs/README.md) for the full documentation hub (SDLC-phase index covering architecture, developer guide, testing strategy, deployment, and operations).

See [`docs/SUMMARY.md`](docs/SUMMARY.md) for the mdbook-style reading order.

## Sibling repos

- [`vmisolate`](../vmisolate) — first consumer (rootfs build path).
- [`justoci`](../justoci) — OCI artifact pipeline; ext4 images are
  one of the artifact types it ships with attestation.
- [`justcas`](../justcas) — content-addressed-storage primitive used
  by `justoci`.

## Background

Originally captured as a deferred design in
[`vmisolate/docs/3-design/adr/019-pure-rust-ext4.md`](../vmisolate/docs/3-design/adr/019-pure-rust-ext4.md).
The deferral was reversed; this repo is the implementation.

## Build

```
cargo build --workspace
cargo test  --workspace
```

Minimum supported Rust version: **1.75**.

## License

[Apache-2.0](LICENSE). Matches `justoci`, `justcas`, OCI specs, and
every CNCF project — same rationale as in
`justoci/docs/0-ideation/research/apache-2-vs-mit.md`.

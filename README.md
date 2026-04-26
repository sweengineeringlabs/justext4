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
- **Write path**: format an empty image, create regular files, allocate
  inodes + data blocks, populate bitmaps correctly.

What's not yet:

- `mkdir`, `unlink`, `truncate`, `rename`, symlinks
- Multi-group images (single group only)
- Block fragmentation (contiguous block allocation only)
- Hash-tree directories (`EXT4_INDEX_FL`)
- Inline-data inodes (`INLINE_DATA` feature)
- Real `mkfs.ext4` fixture round-trip (we test against our own
  formatter today; kernel-mountability not yet verified)
- `METADATA_CSUM` / `64BIT` / journal features

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

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

Pre-v0. Superblock decoding is the first vertical slice. Read API
lands before write. Out of scope for v0: JBD2 journal replay, online
operations, resize / fsck / defragment.

## Crates

| Crate                | Role                                                           |
|----------------------|----------------------------------------------------------------|
| `swe_justext4_spec`  | On-disk format types — superblock, group descriptor, inode, extent, dirent. Pure structs + decode/encode. No IO. |

`swe_justext4_ext4` (read + write API) and `swe_justext4_cli`
(`mkext4-rs` operator binary) land as the spec layer firms up.

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

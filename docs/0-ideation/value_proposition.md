# Value proposition

## Why justext4 exists

The Rust ecosystem has the read half of ext4 covered — the
[`ext4`](https://crates.io/crates/ext4),
[`ext4fs`](https://crates.io/crates/ext4fs), and
[`linux-ext4`](https://crates.io/crates/linux-ext4) crates all parse
existing ext4 images. **None of them produce ext4 images.** Every
shipping pipeline that needs to *create* an ext4 filesystem either:

1. Shells out to `mkfs.ext4` (the `e2fsprogs` user-space tool), or
2. Links against `libext2fs` via FFI, or
3. Builds the image inside a Linux VM and copies the bytes back.

All three paths fail in the cases justext4 is designed for.

## The specific gap

**Pure-Rust, byte-stable, reproducible ext4 image builder.** Three
non-negotiable properties:

### Pure-Rust, no subprocess, no FFI

`mkfs.ext4` is a Linux-only binary. On a Windows host it requires
WSL2; on a macOS host it requires a Linux VM; on a build sandbox
without subprocess permissions (Bazel sandboxed actions, Nix
hermetic builds, locked-down CI runners) it is simply unavailable.
A pure-library path means the same code runs from any platform that
hosts a Rust toolchain, no system tools required. The workspace's
`unsafe_code = "forbid"` lint
([`Cargo.toml:27`](../../Cargo.toml)) keeps the implementation in
a memory-safe sandbox a downstream auditor can sign off on without
re-auditing `e2fsprogs`'s C code.

### Byte-stable output for the same input

`mkfs.ext4` writes a fresh UUID, a fresh hash seed, and the current
wall-clock time into every image it produces. Two invocations on
identical inputs yield two different artifacts. For
reproducible-build pipelines (SLSA, NixOS, Bazel `genrule` outputs)
that's a hard fail: artifact digests drift, downstream caches miss,
and audit checks fall over. justext4 pins the timestamp, UUID, and
hash seed as compile-time constants
([`ext4/src/mkfs.rs:182-191`](../../ext4/src/mkfs.rs)) and asserts
the contract via an always-on test
([`ext4/tests/reproducibility.rs:58-83`](../../ext4/tests/reproducibility.rs)).

### Audit-clean dependency graph

The workspace pulls in exactly one external crate (`thiserror`) for
all three members, declared once in
[`Cargo.toml:24`](../../Cargo.toml). No `bytemuck`, no `zerocopy`,
no proc-macros beyond `thiserror`'s derive. The on-disk format is a
small set of packed binary structs; hand-rolled encode + decode is
~1500 LOC across all the spec types. Fuzz coverage
([`fuzz/`](../../fuzz/)) gives confidence in the manual paths. A
consumer that needs to vet every transitive dep for an air-gapped
or regulated deployment has a single trivially-readable graph to
review.

## Who benefits

- **vmisolate** — first consumer. It uses justext4 to build
  microVM rootfs images on Windows hosts without WSL2, replacing a
  shell-out chain (`mkfs.ext4 + mount + cp -a + umount`) with a
  single subprocess invocation of `mkext4-rs build-from-tree`.
- **Reproducible-image pipelines** — anything wiring ext4 images
  into SLSA-attested or content-addressed artifact systems
  (justoci-style flows). The byte-stability contract means an image
  digest is a fingerprint of the input, full stop.
- **Embedded firmware tooling** — products that ship ext4 images as
  flashable artifacts and want a tiny, auditable build path that
  runs on the developer's laptop without root and without a Linux
  VM.
- **Sandboxed build systems** — Bazel sandboxed actions, Nix
  hermetic derivations, GitHub Actions self-hosted runners on
  Windows: any environment where `subprocess(mkfs.ext4)` is
  unavailable or undesirable.

## Non-goals

These are not what justext4 is and never will be:

- **Not a drop-in `mkfs.ext4` replacement.** justext4 is v0 and
  caps out at single-group images (~128 MiB at 4 KiB blocks),
  fast-symlinks-only (60-byte targets), no journal, no
  `METADATA_CSUM`, no `64BIT` feature. Each gap is tracked under
  the [`v0-gap`](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap)
  GitHub label. For a real distribution rootfs you still want
  `mkfs.ext4`.
- **Not a filesystem driver.** justext4 does not mount, does not
  cache, does not provide a VFS layer. It produces and reads ext4
  byte streams. The kernel mounts what it produces (verified
  manually via `mount -o loop`).
- **Not a write-mounting layer.** There is no journaled,
  crash-consistent, multi-writer story. justext4 writes images
  in-process while holding exclusive ownership of the byte buffer.
  A second writer hitting the same image is undefined.
- **Not a `libext2fs` port.** The implementation is from-scratch
  against the kernel's `fs/ext4/ext4.h` and the
  [kernel.org ext4 layout doc](https://www.kernel.org/doc/html/latest/filesystems/ext4/index.html).
  The output is verified by `e2fsck -nf` (independent
  implementation) and by the Linux kernel's `mount -o loop` path.

## Positioning relative to siblings

justext4 sits in the same "primitive, no deps, reusable" family as
[`justcas`](https://github.com/sweengineeringlabs/justcas) and
[`justoci`](https://github.com/sweengineeringlabs/justoci).
Apache-2.0 licence, mirroring both — see
[`README.md:114-118`](../../README.md).

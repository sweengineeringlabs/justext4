# Market research

**Audience**: Product leads, architects, contributors

Ecosystem facts and producer niche analysis behind justext4's positioning. Two questions answered here: (1) what does the ext4 tooling landscape look like, and (2) who actually needs a pure-Rust, cross-platform ext4 image writer?

---

## Rust ext4 ecosystem survey

Survey of existing ext4 tooling — Rust crates, C tools, and library bindings — and why none covers the write-capable, pure-Rust, cross-platform case that justext4 targets.

### The read-only crates on crates.io

| Crate | Capabilities | Write? | Pure Rust? | Cross-platform? | Notes |
|-------|-------------|--------|------------|-----------------|-------|
| [`ext4`](https://crates.io/crates/ext4) | Superblock, inodes, extents, dir entries, file content | No | Yes | Yes | Most complete read-only crate; no write path at all |
| [`ext4fs`](https://crates.io/crates/ext4fs) | Basic superblock + inode read | No | Yes | Yes | Minimal; stale; no extent tree |
| [`linux-ext4`](https://crates.io/crates/linux-ext4) | On-disk struct definitions | No | Yes | Yes | Struct bindings only; no traversal API |

**Common pattern**: the read half of ext4 has been explored by the Rust ecosystem. Nobody shipped the write half.

#### Why reading is easier than writing

Read-only ext4 is a well-bounded problem: parse the superblock, walk the group descriptor table, follow inodes, decode extent trees, emit bytes. The on-disk format is stable and the happy path is short.

Writing requires:

1. **Allocation** — block bitmap management, inode bitmap management, free-count bookkeeping
2. **Structural consistency** — every write that touches a block must update the bitmap; every new inode must update the inode table *and* the containing directory's entry list *and* the group descriptor's free counts
3. **Atomicity** — partial writes produce corrupt images that `e2fsck` rejects
4. **Reproducibility** — non-deterministic inputs (UUIDs, seeds, timestamps) make the output non-reproducible even when the content is identical

The read-only crates sidestepped all four.

### The C toolchain

#### `mkfs.ext4` / `mke2fs` (e2fsprogs)

The standard tool. Used by every Linux distribution to format ext4 partitions and image files.

| Property | Value |
|----------|-------|
| Platform | Linux only (`/sbin/mkfs.ext4`) |
| Determinism | Non-deterministic by default: injects a random UUID, a random metadata-checksum seed, and embeds the current timestamp |
| Deterministic mode | `mkfs.ext4 -U <uuid> -E lazy_itable_init=0` gets partway there but timestamp injection remains |
| Rust integration | Subprocess only — no library API |
| Windows | Not available without WSL2 or a Linux container |

**Practical implication**: any Rust build tool on Windows that needs to produce a rootfs.ext4 must either bundle WSL2, ship a Linux container as a build dependency, or reimplement the format path.

#### `libext2fs` (part of e2fsprogs)

e2fsprogs ships a C library (`libext2fs`) with a full read/write API. This is what `debugfs`, `resize2fs`, and `tune2fs` use internally.

| Property | Value |
|----------|-------|
| Write capable | Yes — full API |
| Pure Rust | No — C library, requires FFI bindings |
| Cross-platform | Partial — builds on Linux and macOS; Windows support is fragile |
| Rust bindings | None published on crates.io as of survey date |
| Audit surface | Large: 200 KLOC of C, pulling in blkid, uuid, com_err |

A `libext2fs-sys` crate would be feasible but would violate the audit-clean dependency requirement: a C library in the build graph is opaque to `cargo audit` and introduces cross-compilation complexity for Windows targets.

#### `genext2fs`

A standalone C tool (not part of e2fsprogs) specifically designed for producing ext2/ext3/ext4 images from a host directory without requiring root. Lighter than `mkfs.ext4`.

| Property | Value |
|----------|-------|
| Write capable | Yes — format + populate from host dir |
| Pure Rust | No — C binary |
| Cross-platform | Linux / macOS; no native Windows build |
| Deterministic | No — embeds timestamps from source files, uses random UUID |
| Rust integration | Subprocess only |

`genext2fs` is the closest conceptual analogue to justext4's `build-from-tree` command. It inspired the design but doesn't meet the pure-Rust / cross-platform / reproducibility requirements.

### FUSE-based approaches

`fuse-ext4` and similar FUSE filesystems mount an ext4 image as a kernel filesystem, then let userspace tools write to it via the kernel VFS layer.

| Property | Value |
|----------|-------|
| Write capable | Yes — via kernel VFS |
| Pure Rust | No — requires FUSE (Linux) or macFUSE (macOS); not available on Windows |
| Cross-platform | Linux / macOS only |
| Rust integration | Via `fuser` / `fuse-rs` crates — significant complexity |
| Reproducibility | No — kernel timestamps + VFS metadata injection |

FUSE approaches add a kernel boundary with no reproducibility guarantees and no Windows path.

### The gap

Mapping the requirements against the landscape:

| Requirement | `mkfs.ext4` | `libext2fs` FFI | `genext2fs` | FUSE | Existing Rust crates | **justext4** |
|-------------|:-----------:|:---------------:|:-----------:|:----:|:--------------------:|:------------:|
| Write capable | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ |
| Pure Rust | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| Windows native | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| Byte-stable output | ✗ | ✗ | ✗ | ✗ | N/A | ✓ |
| Audit-clean deps | N/A | ✗ | N/A | ✗ | ✓ | ✓ |
| Embeddable as library | ✗ | ✓ | ✗ | ✗ | ✓ | ✓ |

No existing option satisfies all five non-negotiable requirements simultaneously.

### Risk and counter-arguments

**"Just shell out to `mkfs.ext4` and require WSL2."**
Viable for individual developers on Windows. Not viable for CI pipelines where the runner image is Windows Server Core or for reproducible builds where the output must be bit-identical across Linux and Windows builders.

**"Use a Docker container in CI to run `mkfs.ext4`."**
Adds a container runtime as a mandatory build dependency. Complicates hermetic build systems. Still doesn't solve the reproducibility problem (non-deterministic UUID / timestamp injection).

**"Use `libext2fs` via FFI."**
Feasible but adds ~200 KLOC of C to the audit surface, breaks `cargo audit` coverage, and requires a non-trivial cross-compilation setup for Windows. The maintenance burden is ongoing: every e2fsprogs release is a potential ABI break.

**"The existing `ext4` crate will eventually add write support."**
Possible, but it's an unmaintained crate as of the survey date. Waiting is not an option for vmisolate's production timeline.

---

## The ext4 image producer niche

Who needs to produce ext4 filesystem images programmatically — and why the existing toolchain fails them.

### The Docker distinction

A frequent confusion: **Docker the container runtime** is not the same as **ext4 the filesystem**. Docker uses overlayfs (or devicemapper) on top of ext4; it does not produce raw ext4 images. The producers in this niche are building *bootable block devices*, not container layers.

### Concrete producer categories

#### VM image builders on Windows

Projects like vmisolate that build Linux microVM images (`kernel + initrd + rootfs.ext4`) run their build orchestration on Windows hosts. The rootfs must be a valid ext4 image that the Linux guest kernel can mount at boot.

Today's options:

1. Require WSL2 on the developer's machine — excludes Windows Server, CI agents, locked-down corporate laptops
2. Run `mkfs.ext4` in a Linux container — adds Docker as a mandatory build dep
3. Use a pre-built rootfs and skip the format step — limits customisation

justext4 adds a fourth option: call `format()` and `create_file()` directly from the Rust build tool, in process, on Windows.

#### Reproducible-build pipelines

Any pipeline that produces VM images as release artifacts needs the image bytes to be bit-identical given the same inputs — so that:
- The content-addressed digest is stable across rebuilds
- SLSA provenance statements can pin the artifact by digest
- Differential publishing (HEAD-then-PUT) skips re-uploading unchanged images

`mkfs.ext4` injects a random UUID, a random metadata-checksum seed, and the current timestamp into every image. The same source tree produces a different image on every run. justext4 pins all three via `Config`, making the format path deterministic by construction.

#### Embedded firmware teams

Teams shipping ext4-formatted flash images for embedded devices (industrial gateways, NAS appliances, edge compute nodes) typically run a Linux cross-compilation toolchain. But the image packaging step — wrapping the compiled rootfs into an ext4 image — is increasingly happening in CI on Windows or macOS agents.

The same WSL2 / Docker dependency problem applies. The additional constraint here is that the image must be exactly the right size to fit the flash partition, with no wasted blocks — which requires controlling the allocation precisely, not relying on `mkfs.ext4`'s heuristics.

#### Sandboxed build systems

Hermetic build systems (Bazel, Buck2, Nix) enforce that build actions have no network access and no implicit host dependencies. Shelling out to `mkfs.ext4` violates both: it's an undeclared host tool dependency, and on some hosts it doesn't exist.

A pure-Rust library that takes bytes in and bytes out fits cleanly into a hermetic build action. No host tool scanning, no platform conditionals.

#### Rust-based OS / appliance image builders

Projects in the same space as Bottlerocket, Talos, or Flatcar that are building their build tooling in Rust. These teams want the entire image build pipeline in one language with one dependency graph — not a Rust frontend calling out to a C toolchain for the filesystem step.

### What "production-ready" means for this niche

All of the above categories share the same bar:

1. **e2fsck clean.** The output must pass `e2fsck -nf` without errors. This is the kernel's acceptance signal: if e2fsck is happy, the kernel will mount it.
2. **Kernel mountable.** The output must be mountable via `mount -o loop` on Linux ≥ 4.x. e2fsck passing is necessary but not sufficient — some structural issues pass fsck but cause mount failures.
3. **Byte-stable given same inputs.** For content-addressed pipelines and SLSA provenance.
4. **No host tool dependencies.** Pure library — no external binaries, no C FFI.
5. **Embeddable.** Callable from a Rust build tool as a library, not just as a CLI.

The existing toolchain satisfies none of 3, 4, or 5 simultaneously. justext4 satisfies all five.

---

## Footprint

Measured on 2026-04-28, release profile, x86-64 Windows host.
Dep count: `cargo tree -e no-dev --prefix none | sort -u | wc -l` (unique crates, transitive).
Binary size: stripped release binary.

| Tool | Dep count | Binary size | Platform | Notes |
|------|----------:|-------------|----------|-------|
| **justext4** | **15** | **235 KB** | Any | Pure Rust; no C FFI |
| `mkfs.ext4` (e2fsprogs) | ~200 KLOC C | ~1.3 MB (just mkfs.ext4) | Linux only | Entire e2fsprogs suite; random UUID/timestamp by default |
| `genext2fs` | ~5 KLOC C | ~120 KB | Linux/macOS | No Windows build; non-deterministic |
| `libext2fs` (FFI crate, estimated) | ~200 KLOC C (audit-opaque) | N/A (library) | Linux/partial macOS | No published Rust binding; C ABI |

justext4 at 15 transitive crates is the smallest published write-capable Rust ext4 implementation — the only one that exists. The comparison is against C tooling that requires a host installation and is unavailable on Windows.

The CI matrix now includes a `windows-latest` runner. A green badge on `windows-latest` is the machine-verifiable claim; no WSL2, no Docker, no e2fsprogs installed.

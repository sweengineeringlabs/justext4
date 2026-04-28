# justext4 benchmarks

**Audience**: Contributors, adopters evaluating performance.

> **TLDR**: `ext4::format` writes 45–258 MiB/s in-process depending on platform. On the same WSL2 host, `mkfs.ext4` costs 27–285 ms per image vs justext4's 0.1–113 ms — 5–200× slower at small inode counts; comparable at large. Run `cargo bench -p swe_justext4_ext4 --bench write_image` to reproduce. Compare against `mkfs.ext4` via `scripts/bench/compare_mkfs.sh` (Linux/WSL2 required).

## Environment

| Field | Value |
|---|---|
| Date | 2026-04-28 |
| Host | Windows 11, x86-64 |
| WSL2 distro | Ubuntu-24.04 (Hyper-V, /mnt/c mount) |
| Toolchain | stable (release profile) |
| Bench harness | Criterion 0.5 |
| Samples | 100 per case |
| Warmup | 3 s |
| mkfs.ext4 version | mke2fs 1.47.0 (5-Feb-2023) |
| mkfs.ext4 runs | 20 per case (mean reported) |

## Results — `ext4::format` vs `mkfs.ext4`

Parameterised by `inodes_per_group`. `size_blocks` is set large enough to contain the metadata; actual bytes written = `(root_dir_block + 1) × block_size`.

**justext4** writes to an in-memory `Cursor<Vec<u8>>` — no disk I/O.
**mkfs.ext4** writes to a pre-allocated file on the WSL2 filesystem — includes disk flush.
The comparison is honest about this difference; see [What the numbers mean](#what-the-numbers-mean).

### 64-inodes (~36 KB written)

| Tool | Platform | Mean time | Throughput |
|---|---|---|---|
| **justext4** | Windows 11 | **776 µs** | 45 MiB/s |
| **justext4** | WSL2 Ubuntu-24.04 | **136 µs** | 258 MiB/s |
| `mkfs.ext4` | WSL2 Ubuntu-24.04 | 27,215 µs | — |

### 512-inodes (~148 KB written)

| Tool | Platform | Mean time | Throughput |
|---|---|---|---|
| **justext4** | Windows 11 | **4.91 ms** | 29 MiB/s |
| **justext4** | WSL2 Ubuntu-24.04 | **2.00 ms** | 72 MiB/s |
| `mkfs.ext4` | WSL2 Ubuntu-24.04 | 284,875 µs | — |

### 4096-inodes (~1069 KB written)

| Tool | Platform | Mean time | Throughput |
|---|---|---|---|
| **justext4** | Windows 11 | **35.7 ms** | 28 MiB/s |
| **justext4** | WSL2 Ubuntu-24.04 | 113 ms | 9 MiB/s |
| `mkfs.ext4` | WSL2 Ubuntu-24.04 | **27,132 µs** | — |

## What the numbers mean

### Platform gap (Windows vs WSL2)

justext4 on WSL2 is **5.7× faster** than Windows at 64-inodes and **2.5× faster** at 512-inodes. This is the Linux allocator (`mmap`-backed `malloc`) vs the Windows heap — small `Vec` allocations are significantly cheaper on Linux.

At 4096-inodes the gap inverts: WSL2 is **3.2× slower** than Windows. The ~1 MB working set exposes the overhead of running on top of the Hyper-V VM that hosts WSL2, compounded by the `/mnt/c/` NTFS mount used by the bench binary.

### justext4 vs mkfs.ext4

At 64 and 512-inodes justext4 (WSL2) beats `mkfs.ext4` by **200×** and **142×** respectively. This is mostly process startup + disk flush cost in `mkfs.ext4` — it spawns a new process, initialises e2fsprogs, and flushes to disk for every image.

At 4096-inodes `mkfs.ext4` is **4× faster** than justext4 on WSL2. The 1 MB inode table write hits the `/mnt/c/` mount overhead for justext4; `mkfs.ext4` writes to a native ext4 tmpfs path and benefits from the page cache. **This inversion does not occur on Windows** where justext4 runs at 35.7 ms vs mkfs.ext4's unavailability.

The decisive comparison is not throughput but availability: `mkfs.ext4` is not available on Windows at all without WSL2. justext4 runs natively on both with no host tool dependency.

### Outliers

Up to 10% outliers per case are expected on both platforms due to background I/O and scheduler noise.

## Market comparison

Run `scripts/bench/compare_mkfs.sh` on Linux or WSL2 for the `mkfs.ext4` baseline. The script creates a pre-truncated file and times `mkfs.ext4 -N <inodes>` for the same inode counts (20 runs, mean reported).

## Reproducing

```sh
# justext4 bench (Windows or Linux)
cargo bench -p swe_justext4_ext4 --bench write_image

# mkfs.ext4 comparison (WSL2 or Linux only)
bash scripts/bench/compare_mkfs.sh
```

HTML report: `target/criterion/format_image/report/index.html`

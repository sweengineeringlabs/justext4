# justext4 benchmarks

**Audience**: Contributors, adopters evaluating performance.

> **TLDR**: `ext4::format` writes 33–65 MiB/s in-process on an x86-64 Windows dev machine. Run `cargo bench -p swe_justext4_ext4 --bench write_image` to reproduce. Compare against `mkfs.ext4` via `scripts/bench/compare_mkfs.sh` (Linux/WSL2 required).

## Environment

| Field | Value |
|---|---|
| Date | 2026-04-28 |
| Host | Windows 11, x86-64 |
| Toolchain | stable (release profile) |
| Bench harness | Criterion 0.5 |
| Samples | 100 per case |
| Warmup | 3 s |

## Results — `ext4::format` (in-memory, `Cursor<Vec<u8>>`)

Parameterised by `inodes_per_group`, which determines how much inode-table the format function writes. `size_blocks` is set large enough to contain the metadata; actual bytes written = `(root_dir_block + 1) × block_size`.

| Case | Actual bytes written | Mean time | Throughput |
|---|---|---|---|
| 64-inodes | ~36 KB | **553 µs** | 63.5 MiB/s |
| 512-inodes | ~148 KB | **4.43 ms** | 32.6 MiB/s |
| 4096-inodes | ~1069 KB | **29.8 ms** | 34.2 MiB/s |

Outliers: ≤10% per case — typical for a Windows dev machine with background I/O.

## What the numbers mean

- The 64-inode case is dominated by small fixed-overhead writes (superblock, GDT, bitmaps). Throughput is highest here.
- For 512 and 4096 inodes the inode table dominates; throughput stabilises around 33–34 MiB/s.
- All work is in-memory (`Cursor<Vec<u8>>`). File-backed writes will be slower; the comparison point is mkfs.ext4 operating on a pre-allocated file.

## Market comparison

Run `scripts/bench/compare_mkfs.sh` on Linux or WSL2 for the `mkfs.ext4` baseline. The script creates a pre-truncated file and times `mkfs.ext4 -N <inodes>` for the same inode counts. Expected delta: mkfs.ext4 (C, disk-backed) vs justext4 (Rust, in-memory) — not directly comparable on I/O; comparable on CPU-only path when both write to tmpfs.

The differentiator is not raw throughput but:
- **Pure Rust**: no C toolchain, no `e2fsprogs` install, works on Windows without WSL2.
- **Embeddable**: `format<W: Write + Seek>` works on any writer — `Vec`, `File`, a network stream.
- **Auditable**: the entire format path is ~500 lines of safe Rust.

## Reproducing

```sh
cargo bench -p swe_justext4_ext4 --bench write_image
```

HTML report: `target/criterion/format_image/report/index.html`

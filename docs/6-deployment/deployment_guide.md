# Deployment guide

How to consume justext4 from another project. Two integration
patterns exist; pick based on your build pipeline.

## Pattern 1 — Library API (Rust consumers)

Add a path-dep or (once published — see
[issue #12](https://github.com/sweengineeringlabs/justext4/issues/12))
a crates.io version dependency:

```toml
# Cargo.toml in the consuming repo
[dependencies]
ext4 = { path = "../justext4/ext4", package = "swe_justext4_ext4" }
spec = { path = "../justext4/spec", package = "swe_justext4_spec" }
```

Then:

```rust
use std::io::Cursor;
use ext4::{format, Config, Filesystem};

fn build_image() -> std::io::Result<Vec<u8>> {
    let mut img = Vec::new();
    let cfg = Config {
        size_blocks: 4096,         // 16 MiB at default block size
        inodes_per_group: 256,
        volume_label: b"my-rootfs".to_vec(),
        ..Config::default()
    };
    format(&mut Cursor::new(&mut img), &cfg)?;

    let mut fs = Filesystem::open(Cursor::new(&mut img))?;
    fs.mkdir("/etc")?;
    fs.create_file("/etc/hostname", b"my-host")?;
    fs.symlink("/sbin", b"/usr/sbin")?;
    Ok(img)
}
```

Bound `R` is `Read + Seek` for the read API. Add `Write` (e.g.
`Cursor<&mut Vec<u8>>` or `File`) for any of the write methods.

## Pattern 2 — CLI binary (shell pipelines)

Build once, then call from anywhere:

```sh
cargo build --release --manifest-path /path/to/justext4/Cargo.toml \
    --bin mkext4-rs

# Now usable as:
/path/to/justext4/target/release/mkext4-rs build-from-tree \
    ./rootfs ./out.ext4 --inodes 256 --size-blocks 4096 \
    --label my-rootfs
```

Subcommands:

| Subcommand                | Purpose                                                  |
|---------------------------|----------------------------------------------------------|
| `format`                  | Empty filesystem at a given size                         |
| `inspect`                 | Dump superblock + root dir listing                       |
| `touch`                   | Create a regular file inline                             |
| `cat`                     | Read a file's bytes to stdout                            |
| `chmod` / `chown` / `utime` | Mutate inode metadata                                  |
| `build-from-tree`         | Walk a host directory tree and replicate it             |

Use `--help` for the full flag set per subcommand.

## Reference integration: vmisolate

[`vmisolate`](https://github.com/sweengineeringlabs/vmisolate) is the
first consumer.  It uses pattern 2 (CLI binary):

```
vmisolate/
  scripts/
    build-rootfs.sh             # legacy WSL-based builder
    build-rootfs-justext4.sh    # opt-in pure-Rust builder
```

The justext4-based script:

1. Builds `mkext4-rs` from `../justext4` if the binary doesn't exist
2. Materialises a `scratch` skeleton in a tempdir (or accepts a
   `--tree <dir>` argument)
3. Auto-sizes `--size-blocks` from the tree size + 25 % headroom
4. Auto-sizes `--inodes` from the file count + headroom
5. Calls `mkext4-rs build-from-tree`
6. Optionally runs `e2fsck -fnv` for verification (`--verify` flag)

This is the canonical integration shape: the consumer keeps its
existing pipeline, justext4 is opt-in via a side script. As issues
[#1](https://github.com/sweengineeringlabs/justext4/issues/1)
(long symlinks) and
[#9](https://github.com/sweengineeringlabs/justext4/issues/9)
(mknod / device files) close, more vmisolate base modes (busybox,
alpine) become eligible for the justext4 path.

## CI integration

Downstream consumers wanting to validate justext4-built images in CI:

```yaml
# .github/workflows/build.yml in the consumer
- name: Build mkext4-rs
  run: |
    cargo build --release --bin mkext4-rs \
        --manifest-path ../justext4/Cargo.toml

- name: Build rootfs
  run: |
    ../justext4/target/release/mkext4-rs build-from-tree \
        ./rootfs ./out.ext4 --inodes 256 --size-blocks 4096

- name: Verify with e2fsck
  run: |
    sudo apt-get update && sudo apt-get install -y e2fsprogs
    e2fsck -nf ./out.ext4
```

Ubuntu runners have `e2fsprogs` preinstalled, so the install step is
defensive but not strictly required.

## Reproducibility guarantee

`format()` and `build-from-tree` produce byte-identical output for
identical input. Pinned constants (timestamp, UUID, hash seed) make
this hold; an always-on test in `ext4/tests/reproducibility.rs`
asserts the contract. Consumers can compare image digests across
rebuilds to detect input drift — same use case as the SLSA-attested
build pattern in [`justoci`](https://github.com/sweengineeringlabs/justoci).

If the consumer needs a non-pinned timestamp / UUID (e.g., when the
image is meant to be unique-per-build for inventory tracking), the
fields are public on `Superblock` — re-encode after `format()` with
the desired values, or write a thin wrapper that overrides them.

## Platform support

The library + CLI build cleanly on Windows, Linux, and macOS.
The e2fsck-validation tests need `e2fsprogs` on PATH (Linux native or
WSL2 on Windows); they skip-pass otherwise. Kernel-mount validation
is a separate manual step, only meaningful on a Linux host.

## Image-size limits today

| Knob              | v0 cap          | Tracking                                     |
|-------------------|-----------------|----------------------------------------------|
| Total size        | ~128 MiB        | [#4](https://github.com/sweengineeringlabs/justext4/issues/4) (multi-group) |
| Single file       | block_size × extent count × 32767 (no fragmentation across blocks) | [#5](https://github.com/sweengineeringlabs/justext4/issues/5) |
| Single directory  | one block (~340 entries at 4 KiB blocks)  | [#6](https://github.com/sweengineeringlabs/justext4/issues/6) (hash-tree) |
| Inode count       | `inodes_per_group` (single group)         | [#4](https://github.com/sweengineeringlabs/justext4/issues/4) |
| Symlink target    | 60 bytes        | [#1](https://github.com/sweengineeringlabs/justext4/issues/1) (long symlinks) |

For a typical distroless rootfs (~10 MiB, < 50 files, no deeply nested
dirs) all five caps are comfortable. For anything resembling a real
Linux distribution rootfs, you'll hit at least the symlink limit and
likely the file count.

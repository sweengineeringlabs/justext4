# Operations manual

**Audience**: Operators, platform engineers

Operator-facing reference for `mkext4-rs`. Library API users see the
[developer guide](../4-development/developer_guide.md) instead.

## Subcommands at a glance

```
mkext4-rs format <path>
    [--size-blocks N]          # default 16 (64 KiB image)
    [--block-size N]            # 1024 / 2048 / 4096 / 65536; default 4096
    [--label TEXT]              # volume label, ≤ 16 bytes

mkext4-rs inspect <path>
    # dumps the superblock + root directory listing

mkext4-rs touch <image> <vfs-path> <content>
    # create a regular file with literal-string content

mkext4-rs cat <image> <vfs-path>
    # write the file's bytes to stdout (binary-safe)

mkext4-rs chmod <image> <vfs-path> <octal-mode>
    # e.g. 0755; replaces the bottom 12 bits of i_mode

mkext4-rs chown <image> <vfs-path> <uid> <gid>

mkext4-rs utime <image> <vfs-path> <atime-epoch> <mtime-epoch>
    # ctime is bumped automatically (POSIX rule)

mkext4-rs build-from-tree <host-dir> <image>
    [--size-blocks N]
    [--inodes N]                # default 32; larger for real trees
    [--label TEXT]
    # walks the host dir, replicates with mkdir / create_file /
    # symlink. Skips device files + sockets + fifos with a warning
    # (see issue #9).
```

## Common gestures

### Build a rootfs from a host directory

```
mkext4-rs build-from-tree ./rootfs.dir ./out.ext4 \
    --inodes 256 --size-blocks 4096 --label my-rootfs
```

For `inodes`: rule of thumb is `(file count + dir count) × 1.5 + 16`,
floor 64. For `size-blocks`: `(tree-KB × 1.25 + 256)` blocks at 4 KiB.

### Inspect what you produced

```
mkext4-rs inspect ./out.ext4
```

Output:

```
./out.ext4:
  block_size:       4096
  blocks_count:     4096
  free_blocks:      4080
  inodes_count:     256
  free_inodes:      245
  blocks_per_group: 32768
  inodes_per_group: 256
  inode_size:       256
  rev_level:        1
  is_64bit:         false
  volume_label:     "my-rootfs"

/ (4 entries):
  inode=2     type=2 "."
  inode=2     type=2 ".."
  inode=11    type=2 "etc"
  inode=12    type=1 "readme"
```

`type` codes follow the `ext4_dir_entry_2.file_type` field:
`1`=regular file, `2`=directory, `7`=symlink, `0`=type-not-stored
(consult inode mode for the real type).

### Verify with e2fsck

```
# Linux native
e2fsck -nf ./out.ext4

# Windows host with WSL2
wsl -- e2fsck -nf /mnt/c/path/to/out.ext4
```

`-n` (no fixes), `-f` (force check). Exit 0 with no `WARNING` /
`Fix?` lines means the image is kernel-grade clean.

### Mount via the kernel (Linux / WSL2)

```
sudo mkdir -p /mnt/justext4
sudo mount -o loop,ro ./out.ext4 /mnt/justext4
ls /mnt/justext4
sudo umount /mnt/justext4
```

If `mount` succeeds and `ls` shows your tree, the image is real ext4
to the kernel.

## Reading errors

The CLI maps every internal `Ext4Error` to an exit code 1 with a
descriptive stderr line (`error: ...`). The most common operator-
facing errors and what they mean:

| Error fragment                                           | Cause / fix                                                                |
|----------------------------------------------------------|----------------------------------------------------------------------------|
| `superblock layout invalid: <reason>`                    | Image is corrupt or wasn't formatted yet; re-run `format`.                |
| `bad ext4 magic`                                         | The file isn't an ext4 image.                                              |
| `not found: <name>`                                      | Path component doesn't exist; check spelling.                              |
| `is a directory: inode N`                                | You ran `cat` (or `unlink`) on a directory; use a file path or `rmdir`.    |
| `directory not empty: inode N`                           | `rmdir` target has children; remove inner entries first.                   |
| `already exists: <name>`                                 | Collision; pick a different name or remove the existing entry.             |
| `no space: blocks`                                       | Allocator can't find a contiguous run of N blocks. Bump `--size-blocks` or live with the v0 contiguous-only constraint (issue #5). |
| `no space: inodes`                                       | All inodes used. Bump `--inodes`.                                          |
| `unsupported in v0: <detail>`                            | Hit a documented v0 limit. The detail names which (e.g. "directory block full"); cross-reference the issue tracker. |
| `symlink target too long: 76 bytes (max 60)`             | Fast symlinks only; long-symlinks are issue #1.                            |
| `block_size must be 1024, 2048, 4096, or 65536`          | The `--block-size` value is rejected. The kernel only accepts those four. |

## Limits to know about

| Concern                              | v0 behaviour                                                          |
|--------------------------------------|-----------------------------------------------------------------------|
| Image total size                     | ~128 MiB at 4 KiB blocks (single-group cap)                          |
| Directory entry count                | one block worth (~340 entries at 4 KiB blocks). Bigger = `UnsupportedV0` |
| Symlink target length                | 60 bytes max (fast-symlink only). Bigger = `SymlinkTargetTooLong`     |
| Block fragmentation after churn      | `NoSpace` even when total free ≥ N if no contiguous run of N exists  |
| Device files in the host tree        | Skipped with a warning during `build-from-tree`                       |
| Hash-tree directories on read        | Linear walk only — large dirs return only the first block's entries  |
| Inline-data inodes on read           | Treated as having empty extent tree; reads return wrong content      |

Each is tracked as a labeled GitHub issue under
[`v0-gap`](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap).
Hitting a limit doesn't corrupt; it returns a typed error early.

## Determinism: same input, same bytes

`format` and `build-from-tree` produce byte-identical output for
identical input. Pinned constants for timestamp, UUID, and the
directory hash seed make this hold. Two consequences:

1. **Image digests are reproducible** across rebuilds. Useful for
   "did anything change?" checks in CI without re-running fsck.
2. **Operators expecting unique UUIDs across distinct images are
   surprised**. If you build two images with the same `Config`, they
   share the same `Filesystem UUID` field. Override the `Superblock`'s
   `uuid` post-format if your downstream needs uniqueness.

The reproducibility test in `ext4/tests/reproducibility.rs` pins
the contract.

## When something goes wrong

1. Run `mkext4-rs inspect <image>` first — sanity-check the
   superblock and root listing match expectations.
2. Run `e2fsck -nf <image>` — if it surfaces a specific corruption
   (free counts wrong, links_count wrong, etc.), that's a justext4
   bug. File an issue with the e2fsck output and the input that
   produced it.
3. For mount failures on Linux, `dmesg` after the failed `mount`
   often has a precise reason ("ext4: bad block group N",
   "EXT4-fs: couldn't mount because of unsupported optional
   features"). The latter usually means feature-bit drift between
   what we emit and what the kernel expects.
4. For `cargo test --workspace` failures locally that don't repro
   on CI: ensure your shell isn't mangling VFS paths (`/foo` →
   `C:/Program Files/Git/foo` on Git Bash). The CLI's path-translation
   helper handles this; tests use `Cursor`-backed in-memory images
   to avoid the trap entirely.

## Reporting bugs

GitHub: https://github.com/sweengineeringlabs/justext4/issues

For format-correctness bugs, include:
- The exact command(s) that produced the broken image
- `mkext4-rs inspect <image>` output
- `e2fsck -nf <image>` output (or `wsl -- e2fsck ...` on Windows)
- The Cargo.lock at the time (so the reproducer pins decoder versions)

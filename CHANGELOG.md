# Changelog

All notable changes to justext4 are documented here.

## v0 — current

Initial working release.

**Read path**: superblock, group descriptors, inodes, extent trees, directory entries, file content, path resolution.

**Write path**: `format` / `create_file` / `mkdir` / `unlink` / `rmdir` / `symlink` / `truncate` / `rename` / `chmod` / `chown` / `utime`. Bitmap allocator. Build-from-host-tree.

**Kernel interop**: reads real `mkfs.ext4` output (committed fixture); output passes `e2fsck -nf` clean and is mountable via `mount -o loop`.

**Reproducibility**: same `Config` produces byte-identical output across runs (pinned by an always-on test).

**Fuzz harness**: five `cargo-fuzz` targets in `fuzz/` (one per spec-layer decoder).

**Known gaps** (tracked as GitHub issues with `v0-gap` label): long symlinks, append-to-existing-file, xattr, mknod, multi-group images, block fragmentation, hash-tree directories, inline-data inodes, JBD2 journal, crates.io publication.

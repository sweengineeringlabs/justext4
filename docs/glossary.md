# Glossary

**Audience**: All

Alphabetized list of terms used in justext4.

---

**bitmap** - A block-granularity bitfield tracking which blocks (block bitmap) or inodes (inode bitmap) in a block group are allocated. justext4 maintains separate block and inode bitmaps per group.

**block** - The fundamental allocation unit of ext4. justext4 v0 uses 4 KiB blocks exclusively. Block addresses are 32-bit in single-group mode.

**block group** - A region of the ext4 image containing a fixed number of blocks, with its own group descriptor, block bitmap, inode bitmap, and inode table. justext4 v0 is single-group only (~128 MiB cap at 4 KiB blocks).

**block group descriptor (BGD)** - An on-disk struct describing one block group: block bitmap location, inode bitmap location, inode table location, free block count, free inode count, used directory count.

**CAS** - Content-Addressed Storage. A pattern for storing blobs by the hash of their content rather than by a mutable name. Not used directly by justext4, but the byte-stability guarantee makes justext4-produced images compatible with CAS-backed pipelines.

**checksum** - ext4 uses metadata checksums (`METADATA_CSUM` feature) for superblock, group descriptors, bitmaps, and inodes. justext4 v0 does not enable `METADATA_CSUM`; `e2fsck -nf` accepts the output without it.

**contiguous-block allocator** - justext4's v0 block allocator always writes file data in a single contiguous run. No fragmentation handling. This is intentional for reproducibility: same input → same block layout → same image bytes.

**`Cursor<Vec<u8>>`** - The typical IO type used in justext4 tests: an in-memory buffer implementing `Read + Write + Seek`. Allows full format-then-read roundtrips without touching the filesystem.

**DirEntry** - An on-disk ext4 directory entry: inode number + record length + name length + file-type byte + variable-length filename. justext4 v0 uses the linear (non-hash-tree) format.

**e2fsck** - The ext4 filesystem checker from `e2fsprogs`. justext4 uses `e2fsck -nf` as a kernel-compatibility acceptance gate: output that passes e2fsck clean is considered correct. The check runs on every CI push.

**extent** - ext4's mechanism for mapping logical file blocks to physical disk blocks. An extent covers a contiguous range: `(logical_block, physical_block, length)`. justext4 v0 uses the inline extent tree (up to 4 extents per inode, no additional extent blocks needed for v0 files).

**extent tree** - The tree structure in an inode that maps logical blocks to physical extents. justext4 v0 uses depth-0 (inline) extent trees only.

**fast symlink** - A symlink whose target fits in the inode's block-pointer array (≤ 60 bytes in ext4). justext4 v0 supports fast symlinks only; long symlinks (> 60 bytes) are a tracked gap.

**`Filesystem<R>`** - The central type in justext4's `ext4` crate. Generic over its IO backend (`R: Read + Write + Seek`). Owns the on-disk state and exposes the read/write API.

**`format`** - The entry point that writes a fresh ext4 image: superblock, group descriptor table, block bitmap, inode bitmap, inode table, root directory inode, and lost+found directory. Deterministic — same `Config` → same bytes.

**GDT (Group Descriptor Table)** - The table of block group descriptors, written after the superblock. In single-group images, one entry.

**inode** - An on-disk metadata node for a file or directory: permissions, timestamps, size, block pointers (extent tree). Each file and directory has exactly one inode. Identified by inode number.

**inode table** - The region within a block group that stores all inodes for that group. Fixed size at format time.

**`mkext4-rs`** - The CLI binary provided by justext4 (`swe_justext4_cli` crate). Wraps the `ext4` crate's format/read/write API for shell use.

**MSRV** - Minimum Supported Rust Version. justext4 targets Rust 1.75; CI enforces this with a dedicated check step.

**skip-pass** - A test pattern used for tests that require external tools (e2fsck, mount, dumpe2fs). The test always compiles and runs; when the required env var is unset it prints `SKIP` and passes. When the env var is set it runs the real check. Prevents CI blocking on tool availability.

**spec crate** (`swe_justext4_spec`) - The bottom crate in the workspace: on-disk format types only. No IO, no allocation logic. Symmetric encode/decode for every structure ensures the test suite can decode what the write path encoded.

**superblock** - The top-level ext4 metadata structure at byte offset 1024. Contains filesystem size, block size, feature flags, UUID, label, and counts. justext4 writes a minimal superblock with only the features required for v0.

**UUID** - A 128-bit identifier stored in the ext4 superblock. justext4 pins the UUID to a deterministic value from `Config` (default: all-zeros) to ensure byte-stable output. Operators who need unique UUIDs per image must set `Config::uuid`.

---

## See Also

- [Architecture](3-design/architecture.md)
- [Deployment guide](6-deployment/deployment_guide.md)
- [Operations manual](6-deployment/operations_manual.md)

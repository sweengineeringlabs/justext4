# Developer guide

## Setup

```
git clone git@github.com:sweengineeringlabs/justext4.git
cd justext4
cargo build --workspace
cargo test  --workspace
```

MSRV: **1.75**. CI runs `cargo check --workspace --all-targets` on
1.75 on every push. Going lower needs review: nothing in the active
dep graph blocks 1.75 today.

The repo is self-contained. No path-deps on sibling repos at this
point (cross-repo integration happens at the consumer side, e.g.
vmisolate calls `mkext4-rs` via subprocess).

## Build

```
cargo build --workspace                  # debug
cargo build --workspace --release        # release; mkext4-rs is the
                                         # only binary that benefits
cargo run --bin mkext4-rs -- --help      # operator CLI smoke test
```

The `fuzz/` directory is its own sub-project (cargo-fuzz convention),
not a workspace member. Build with:

```
cd fuzz && cargo +nightly fuzz build
```

Requires `cargo install cargo-fuzz` and a nightly toolchain.

## Test

```
cargo test --workspace
```

Runs everything by default. The two integration test files under
`ext4/tests/` exercise different surfaces:

- `real_mkfs_roundtrip.rs` — opens a committed `mkfs.ext4`-produced
  fixture (`ext4/tests/fixtures/real_minimal.ext4`) and walks it.
  Validates the read path against bytes an independent
  implementation actually wrote.
- `e2fsck_acceptance.rs` — produces images with our `format()` /
  `create_file` / `mkdir` / etc., runs `e2fsck -nf` against them,
  asserts kernel-grade fsck accepts them clean.

The e2fsck test is **always-on, skip-pass mode**. When `e2fsck`
isn't reachable on the host, it prints `SKIP` to stderr and passes.
Detection order:

1. `which e2fsck` succeeds → run directly
2. `JUSTEXT4_E2FSCK_VIA_WSL=1` env var set → run via `wsl -- e2fsck`,
   translating the Windows tempdir path to `/mnt/c/...`
3. Otherwise → print SKIP, pass

On a Windows dev box with WSL2:

```
JUSTEXT4_E2FSCK_VIA_WSL=1 \
    cargo test -p swe_justext4_ext4 --test e2fsck_acceptance
```

On Ubuntu (CI runners ship `e2fsprogs`), the test runs directly without
the env var.

## Lint + format

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Both are CI gates. The `RUSTFLAGS=-D warnings` workflow-level env
turns any warning into a build failure across all targets including
tests.

## Adding a new write op

Mirror the shape of an existing op (`create_file`, `mkdir`, `unlink`).
The pattern, in order:

1. **Resolve the path** via `Self::split_path` + `open_path`.
2. **Read the parent inode**; assert `is_directory()`.
3. **Lookup** to detect collision (`AlreadyExists`) or absence
   (`NotFound`).
4. **Allocate** any new resources (`allocate_inode`,
   `allocate_blocks_contiguous`).
5. **Build** the new inode struct (`mode`, `links_count`, `flags`,
   `block`, `size`, ...).
6. **Persist** via `write_inode` for inodes, raw seek+write for data
   blocks.
7. **Update directory state** via `add_dir_entry` /
   `remove_dir_entry`.
8. **Update parent metadata** (e.g., bump `links_count` for new
   subdirs, bump GDT `used_dirs_count`).
9. **Call `flush_metadata`** to persist the in-memory superblock +
   GDT deltas.

Every op needs:

- A doc comment describing semantics + the bug class it prevents
- Unit tests in `ext4/tests/<op>_tests.rs` (NOT in `mkfs.rs` — keeps
  agents out of a hot file when they fan out)
- An e2fsck regression test appended to `ext4/tests/e2fsck_acceptance.rs`
  (mirror the existing `detect_runner` / `build_command` pattern)
- Each test names the bug it catches in its doc comment

## Test conventions

```rust
/// `<short summary of what's tested>`.
///
/// Bug it catches: <specific regression that would surface>.
/// <How the test would surface it.>
#[test]
fn test_<action>_<condition>_<expectation>() {
    // setup
    let ...;
    // trigger
    let result = ...;
    // assert
    assert_eq!(result, expected, "...");
}
```

Test names follow `test_<action>_<condition>_<expectation>`.
Smoke tests that just check "the function exists and runs without
panicking" are **not allowed** — every test must name a bug it
catches in a doc comment, and every assertion must be specific
enough to fail when that bug surfaces. The cardinal rule.

## Six-branch flow

The repo uses the same six-branch flow as `vmisolate`:

```
dev → test → int → uat → prd → main
```

`dev` is the default. Slices commit on `dev`, push, then cascade via
fast-forward merges through each downstream branch. CI runs on every
branch on every push.

For agent-spawned work: each agent gets its own worktree of the
repo (`git worktree add ../justext4-worktrees/<feature> -b feature/<name> dev`),
commits on the feature branch, pushes. The parent merges + cascades
sequentially when all agents complete. Worktrees + feature branches
are cleaned up after the merge lands.

## Issue tracking

Unowned ext4 features (multi-group, hash-tree, xattr, journal, ...)
are tracked as labeled GitHub issues — see
[`v0-gap`](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap).
The README's "What's not yet" cross-references these. New gaps go
in this label set.

## Fuzzing

```
cd fuzz
cargo install cargo-fuzz   # one-time
cargo +nightly fuzz run superblock_decode
cargo +nightly fuzz run inode_decode
cargo +nightly fuzz run dir_block_decode
cargo +nightly fuzz run extent_node_decode
cargo +nightly fuzz run group_descriptor_decode
```

Each target asserts "no panic on arbitrary input" through the
corresponding spec decoder. Corpora and crashes are not committed
(`.gitignore` covers `fuzz/target/`, `fuzz/corpus/`, `fuzz/artifacts/`).

There's no requirement to run fuzzers as part of the default test
suite — they're slow and need a separate process.

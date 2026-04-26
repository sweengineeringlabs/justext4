# justext4 fuzz harness

cargo-fuzz targets that feed arbitrary `&[u8]` data into each public
decoder in `swe_justext4_spec` and assert no panic. The contract
under test is "decoders surface a typed error for any input" — the
implementations should never `unwrap`, never index out of bounds,
never overflow.

## Layout

```
fuzz/
├── Cargo.toml                        # sub-project — NOT a member of the
│                                     # parent workspace; lives in its own
│                                     # cargo invocation per cargo-fuzz convention
├── fuzz_targets/
│   ├── superblock_decode.rs          # Superblock::decode(&[u8])
│   ├── inode_decode.rs               # Inode::decode(&[u8], &Superblock)
│   ├── dir_block_decode.rs           # decode_dir_block(&[u8])
│   ├── extent_node_decode.rs         # decode_extent_node(&[u8])
│   └── group_descriptor_decode.rs    # GroupDescriptor::decode(&[u8], &Superblock)
└── README.md                         # this file
```

The `fuzz/` directory is **not** a member of the root workspace — it
has its own `Cargo.toml` and is built independently. This is the
cargo-fuzz convention: it keeps the libfuzzer-sys dependency and the
nightly-only build flags off the main crate graph.

For the two targets that need a `Superblock`
(`inode_decode`, `group_descriptor_decode`), the harness synthesizes
a minimal valid superblock once via `Superblock::decode` and reuses
it across calls. The fuzzer only mutates the per-target payload, not
the superblock shape.

## Prerequisites

- Nightly Rust toolchain — `rustup toolchain install nightly`
- cargo-fuzz — `cargo install cargo-fuzz`

cargo-fuzz drives libFuzzer, which is shipped with rustc nightly.

## Running

From this directory (`fuzz/`):

```bash
cd fuzz
cargo +nightly fuzz run superblock_decode
cargo +nightly fuzz run inode_decode
cargo +nightly fuzz run dir_block_decode
cargo +nightly fuzz run extent_node_decode
cargo +nightly fuzz run group_descriptor_decode
```

Each invocation runs forever until a crash is found or you Ctrl-C.
For a bounded run (CI), pass `--`-forwarded libFuzzer flags:

```bash
cargo +nightly fuzz run superblock_decode -- -max_total_time=60
```

## Triage

When libFuzzer finds a crashing input it writes the bytes to
`fuzz/artifacts/<target>/crash-<sha>` and prints the path. Reproduce
with:

```bash
cargo +nightly fuzz run superblock_decode fuzz/artifacts/superblock_decode/crash-<sha>
```

## Corpora

Generated corpora land under `fuzz/corpus/<target>/`. They are not
committed — they balloon over time and are not deterministic. Seed
the corpus by hand if you want directed coverage (e.g. drop a known
good superblock into `fuzz/corpus/superblock_decode/`); cargo-fuzz
will pick it up on the next run.

## Smoke build (no fuzzer)

To verify the harness compiles without running the fuzzer (e.g. to
catch a wiring break in CI on a stable toolchain):

```bash
cargo build --manifest-path fuzz/Cargo.toml
```

This builds the targets as plain bins linked against `libfuzzer-sys`
without enabling the libfuzzer instrumentation. It catches API drift
between `swe_justext4_spec` and the harness.

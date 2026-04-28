# Contributing to justext4

## Branch model

Six-branch flow: `dev` → `test` → `int` → `uat` → `prd` → `main`. PRs target `dev`. Feature branches branch from `dev`.

## Before opening a PR

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

All three must pass. CI enforces `RUSTFLAGS=-D warnings`.

## Test conventions

Every test must follow `test_<action>_<condition>_<expectation>` naming and include a `// Bug it catches:` doc comment naming the specific regression it guards. See [`docs/4-development/developer_guide.md`](docs/4-development/developer_guide.md) for the full convention.

## Adding a write operation

Follow the nine-step recipe in the developer guide: spec type → encode/decode → unit test → ext4 write → integration test → e2fsck skip-pass → CLI subcommand → documentation.

## Reporting bugs

Open a GitHub issue. For unimplemented v0 features, use the `v0-gap` label.

## License

By contributing you agree your contributions will be licensed under [Apache-2.0](LICENSE).

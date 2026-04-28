# Compliance Checklist

**Audience**: Contributors, architects, code reviewers

Use this checklist during code review for any PR that touches justext4's architectural boundaries. Derived from `docs/3-design/architecture.md`. Every item must pass before merge.

---

## How to Use

1. Open this checklist alongside the PR diff
2. Run the automated checks where provided
3. Mark each item pass/fail; note the failing file
4. Fix failures before merge — structure violations first

---

## 1. Crate Layering Compliance

Reference: `docs/3-design/architecture.md` — "Three crates, one direction"

### 1.1 spec is IO-free

- [ ] `spec` crate has no `use std::io` or `use std::fs` in non-test code
- [ ] `spec` crate does not depend on `ext4` or `cli` crates
- [ ] All on-disk types (Superblock, Inode, Extent, DirEntry) live in `spec`, not in `ext4`

### 1.2 ext4 owns all IO

- [ ] `ext4` crate does not call `std::process::Command` (no subprocess spawning)
- [ ] `ext4` crate does not use FFI (`extern "C"` blocks)
- [ ] All filesystem operations go through `Filesystem<R>` — no bare file writes bypassing the struct

### 1.3 cli is thin

- [ ] `cli` crate contains no business logic — only argument parsing + `ext4` crate calls
- [ ] No ext4 on-disk format types defined in `cli` — all from `spec`

**Verify**:
```bash
# spec must not import IO
grep -rn 'use std::io\|use std::fs' crates/spec/src/ --include="*.rs" | grep -v '#\[cfg(test)\]' | grep -v 'tests/' && echo "FAIL" || echo "PASS"
# ext4 must not spawn subprocesses
grep -rn 'std::process::Command' crates/ext4/src/ --include="*.rs" | grep -v 'tests/' && echo "FAIL" || echo "PASS"
```

---

## 2. Reproducibility Compliance

Reference: `docs/3-design/architecture.md` — "Byte-stable output"

### 2.1 No non-deterministic values in format path

- [ ] No `SystemTime::now()` in the format path (timestamps come from `Config`)
- [ ] No `rand` or UUID generation that isn't seeded from `Config`
- [ ] No `HashMap` in any type that contributes to on-disk layout (use `BTreeMap` or fixed-order arrays)

### 2.2 Reproducibility test

- [ ] The always-on reproducibility test exists and runs in CI (not `#[ignore]`-gated)
- [ ] The test formats twice with the same `Config` and asserts byte-identical output

**Verify**:
```bash
# No SystemTime::now in format path
grep -rn 'SystemTime::now\|Instant::now' crates/ext4/src/ crates/spec/src/ --include="*.rs" | grep -v 'test' && echo "FAIL" || echo "PASS"
# Reproducibility test exists and is not #[ignore]
grep -rn 'reproducib' crates/ --include="*.rs" | grep -v '#\[ignore\]' | head -5
```

---

## 3. Symmetric Encode/Decode Compliance

Reference: `docs/3-design/architecture.md` — "spec: symmetric encode/decode"

- [ ] Every struct in `spec` that has an `encode` (or `to_bytes`) also has a `decode` (or `from_bytes`)
- [ ] Every struct that has a `decode` also has an `encode`
- [ ] The roundtrip property is tested: `decode(encode(x)) == x` for each struct

**Verify**:
```bash
# Each spec type should have both encode and decode
grep -rn 'fn encode\|fn to_bytes' crates/spec/src/ --include="*.rs"
grep -rn 'fn decode\|fn from_bytes' crates/spec/src/ --include="*.rs"
```

---

## 4. Pure-Rust Compliance

Reference: `docs/3-design/architecture.md` — "Pure Rust: no subprocess, no FFI"

- [ ] No `build.rs` that links C libraries
- [ ] No `extern "C"` blocks in `spec` or `ext4` crates
- [ ] No `std::process::Command` in `spec` or `ext4` crates (only allowed in tests that call `e2fsck`/`mount`)
- [ ] Cargo.toml dependency list contains no C-wrapper crates (`-sys` suffix crates)

**Verify**:
```bash
# No sys crates in core dependencies
grep -E '"-sys"' crates/spec/Cargo.toml crates/ext4/Cargo.toml && echo "FAIL" || echo "PASS"
# No extern C in non-cli crates
grep -rn 'extern "C"' crates/spec/src/ crates/ext4/src/ --include="*.rs" && echo "FAIL" || echo "PASS"
```

---

## 5. Test Quality Compliance

Reference: `docs/4-development/developer_guide.md` — "Test conventions"

- [ ] Every new test follows `test_<action>_<condition>_<expectation>` naming
- [ ] Every new test has a `// Bug it catches:` doc comment
- [ ] No test uses `unwrap()` or `expect()` without an inline comment explaining why it cannot fail
- [ ] Skip-pass tests (e2fsck, mount, dumpe2fs) check for env var before calling the external tool

**Verify**:
```bash
# Find tests missing "Bug it catches:" comment (approximate)
grep -rn '#\[test\]' crates/ --include="*.rs" -A3 | grep -v 'Bug it catches' | grep 'fn test_' | head -20
```

---

## 6. Security Compliance

- [ ] No credentials, tokens, or keys committed to VCS
- [ ] Error messages do not expose internal memory addresses or stack traces to CLI output
- [ ] Filesystem paths from CLI arguments are not shell-expanded or passed to `sh -c`

---

## Quick Summary Table

| Category | Checks | Pass | Fail |
|----------|--------|------|------|
| Crate layering | 1.1–1.3 | | |
| Reproducibility | 2.1–2.2 | | |
| Symmetric encode/decode | 3 | | |
| Pure-Rust | 4 | | |
| Test quality | 5 | | |
| Security | 6 | | |
| **Total** | | | |

---

## Automated Gate

```bash
# Build (catches dep violations, type mismatches)
cargo build --workspace

# Full test suite
cargo test --workspace

# Clippy — errors only
cargo clippy --workspace -- -D warnings

# Format
cargo fmt --check
```

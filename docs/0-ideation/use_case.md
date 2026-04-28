# Use cases

**Audience**: Product leads, architects, integrators

Concrete actor + action + outcome descriptions for justext4. Each use case names who is doing what, under what constraints, and what a successful outcome looks like.

---

## UC-01: Windows CI pipeline builds a Linux microVM rootfs

**Actor**: Platform engineering team building Linux microVM images from a Windows CI runner.

**Context**: The team produces `rootfs.ext4` images for microVMs. Their CI runs on Windows Server (no WSL2, no Docker). Previously they required a Linux sidecar container to run `mkfs.ext4`, adding Docker as a mandatory build dependency and doubling CI setup time.

**Flow**:
1. Build step produces the rootfs directory tree (binaries, configs, init).
2. Rust build tool calls `justext4::format()` then populates via `create_file()` / `mkdir()` — in process, no subprocess.
3. The resulting `rootfs.ext4` bytes are written to `dist/`.
4. The VM image pipeline picks up `dist/rootfs.ext4` as a layer.

**Outcome**: `rootfs.ext4` produced on a Windows runner without WSL2 or Docker. Same image bytes on Windows and Linux builders given the same input tree.

**Constraints satisfied**: Pure Rust — no `mkfs.ext4` on PATH, no Linux container, no root privileges required.

---

## UC-02: Reproducible-build pipeline pins VM image digests

**Actor**: Release engineering team that produces VM images as SLSA-attested artifacts.

**Context**: The team needs the `rootfs.ext4` digest to be stable across rebuilds so that SLSA provenance statements can pin it by hash. `mkfs.ext4` injects a random UUID, metadata-checksum seed, and current timestamp into every image — the same source tree produces a different digest on every run, breaking content-addressed pipelines.

**Flow**:
1. Build produces the same source tree on every run (reproducible build inputs).
2. `justext4::format()` is called with a fixed `Config` (pinned UUID, timestamp, hash seed).
3. Two builds of the same tree produce byte-identical `rootfs.ext4` files.
4. SLSA provenance statement pins the image by digest. Differential publish (HEAD-then-PUT) skips re-upload when nothing changed.

**Outcome**: Artifact digest is a pure function of input content. Provenance statements are stable. Cache hit rates improve.

**Constraints satisfied**: Byte-stability guaranteed by construction and enforced by an always-on test. No configuration required beyond using the default `Config`.

---

## UC-03: Embedded firmware team packages a flash partition image

**Actor**: Embedded systems team shipping ext4-formatted flash images for an industrial gateway.

**Context**: The flash partition must be exactly the right size (no wasted blocks) and reproducible across builds. The team cross-compiles on macOS and Windows CI agents. `mkfs.ext4`'s heuristic block allocation and non-deterministic metadata make it unsuitable.

**Flow**:
1. Cross-compilation produces the target rootfs tree.
2. `Config::image_size_bytes` is set to the exact flash partition size.
3. `justext4::format()` allocates blocks precisely, with no slack.
4. `e2fsck -nf` runs as a CI gate to confirm the image is kernel-acceptable before flashing.

**Outcome**: Flash image is exactly the right size, reproducible, and passes the kernel acceptance check on every CI run.

**Constraints satisfied**: No Linux required on the build host. Partition size is a first-class config parameter, not a heuristic.

---

## UC-04: Hermetic build system produces an ext4 image as a build output

**Actor**: Platform team using Bazel or Nix with hermetic build constraints.

**Context**: Hermetic build systems require all inputs to be declared. A `genrule` that shells out to `mkfs.ext4` is an undeclared host-tool dependency — it breaks the hermeticity contract and fails in sandboxed actions where subprocess execution is prohibited.

**Flow**:
1. A Rust build action declares `swe_justext4_ext4` as a `Cargo.toml` dependency — fully declared, tracked by lockfile.
2. The action calls `format()` + write ops entirely in-process.
3. The output `rootfs.ext4` is a declared build output with a stable digest.

**Outcome**: ext4 image production fits cleanly into hermetic build rules. No host-tool scanning, no platform conditionals, no undeclared dependencies.

**Constraints satisfied**: Pure library — takes bytes in, emits bytes out. No network access, no filesystem side effects beyond the output buffer.

# justext4 docs

Organised by SDLC phase, mirroring the
[`justoci`](https://github.com/sweengineeringlabs/justoci) sibling.

- [`3-design/architecture.md`](3-design/architecture.md) — the three-
  crate layout, read + write data flow, key design decisions
  (pure-Rust, byte-stable output, single-group v0 cap).
- [`4-development/developer_guide.md`](4-development/developer_guide.md)
  — setup, build/test/lint commands, conventions for adding a write
  op, six-branch flow, fuzzing.
- [`6-deployment/deployment_guide.md`](6-deployment/deployment_guide.md)
  — two integration patterns (library API vs CLI binary), reference
  integration in vmisolate, downstream CI shape, image-size limits.
- [`7-operations/operations_manual.md`](7-operations/operations_manual.md)
  — operator-facing CLI reference, common gestures, error map,
  v0 limits, troubleshooting.

The top-level [`README.md`](../README.md) is the project's elevator
pitch and "what's working / what's not" status. The unowned ext4
features are tracked as labeled
[GitHub issues](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap).

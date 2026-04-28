# justext4 docs

**Audience**: All

Organised by SDLC phase.

- [`0-ideation/value_proposition.md`](0-ideation/value_proposition.md) — what problem justext4 solves, who it's for, and the alternatives.
- [`0-ideation/market_research.md`](0-ideation/market_research.md) — Rust ext4 ecosystem survey (existing crates, C tooling, gap matrix) and producer niche analysis (who needs a pure-Rust ext4 writer and why).
- [`3-design/architecture.md`](3-design/architecture.md) — the three-
  crate layout, read + write data flow, key design decisions
  (pure-Rust, byte-stable output, single-group v0 cap).
- [`4-development/developer_guide.md`](4-development/developer_guide.md)
  — setup, build/test/lint commands, conventions for adding a write
  op, six-branch flow, fuzzing.
- [`6-deployment/deployment_guide.md`](6-deployment/deployment_guide.md)
  — two integration patterns (library API vs CLI binary), reference
  integration in vmisolate, downstream CI shape, image-size limits.
- [`6-deployment/operations_manual.md`](6-deployment/operations_manual.md)
  — operator-facing CLI reference, common gestures, error map,
  v0 limits, troubleshooting.

The top-level [`README.md`](../README.md) is the project's elevator
pitch and "what's working / what's not" status. The unowned ext4
features are tracked as labeled
[GitHub issues](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap).

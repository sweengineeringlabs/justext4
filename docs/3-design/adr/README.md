# Architecture Decision Records

**Audience**: Contributors, architects

Architecture Decision Records for justext4. Each ADR documents a significant design choice, the context that drove it, and the trade-offs accepted.

## Format

Files follow `NNN-title.md` naming (zero-padded number, snake_case words). Status values: `Accepted`, `Superseded by ADR-NNN`, `Deprecated`.

## Index

No ADRs recorded yet. Key decisions made during v0 are captured inline in the design documents:

| Decision | Location |
|----------|----------|
| Pure Rust — no subprocess, no FFI | [`architecture.md` §"Key design decisions"](../architecture.md) |
| Byte-stable output (pinned timestamps, UUID, hash seed) | [`architecture.md` §"Byte-stable output"](../architecture.md) |
| Single-group v0 cap (~128 MiB) | [`architecture.md` §"Single-group v0"](../architecture.md) |
| Contiguous-only block allocator | [`architecture.md` §"Contiguous-only allocation"](../architecture.md) |
| No bytemuck/zerocopy deps | [`architecture.md` §"No bytemuck/zerocopy"](../architecture.md) |
| Symmetric encode/decode in spec crate | [`architecture.md` §"spec: symmetric encode/decode"](../architecture.md) |
| `Filesystem<R>` generic over IO | [`architecture.md` §"Filesystem<R>"](../architecture.md) |
| Project origin: vmisolate ADR-019 deferral reversed | [`vmisolate/docs/3-design/adr/`](../../../vmisolate/docs/3-design/adr/) |

When a decision warrants its own ADR, add `NNN-decision_title.md` here and link it in this index.

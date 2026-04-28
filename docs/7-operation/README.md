# Operation

**Audience**: Operators, platform engineers

Runbooks and operational guidance for using justext4 in production pipelines.

## Status

`mkext4-rs` is a CLI tool used in build pipelines, not a long-running service. The primary operational reference is the operations manual in `6-deployment/`. This directory will hold runbooks as deployment patterns and failure modes mature.

## Contents

| Artifact | Status | Description |
|----------|--------|-------------|
| `runbook.md` | Not yet written | Pipeline integration runbook |

## Interim References

- **CLI reference and error map**: [`docs/6-deployment/operations_manual.md`](../6-deployment/operations_manual.md) — subcommand reference, v0 limits, error codes, troubleshooting
- **Integration patterns**: [`docs/6-deployment/deployment_guide.md`](../6-deployment/deployment_guide.md) — library API vs CLI binary, CI integration
- **Kernel verification**: `e2fsck -nf <image>` — accepts all v0 justext4 output; see the testing strategy for the skip-pass pattern

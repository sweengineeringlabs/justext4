# Planning

**Audience**: Contributors, project leads

Sprint planning, RFCs, and progress tracking for justext4.

## Status

v0 planning is tracked via GitHub issues (label `v0-gap` for unimplemented features). This directory will hold RFCs and sprint docs as the project grows beyond the initial v0 scope.

## Contents

| Artifact | Status | Description |
|----------|--------|-------------|
| `rfc/` | Not yet created | Request for Comments — pre-decision proposals |
| `decision_log.md` | Not yet written | Cross-cutting decision log |

## Interim References

- **Open features**: GitHub issues labeled [`v0-gap`](https://github.com/sweengineeringlabs/justext4/issues?q=label%3Av0-gap)
- **Key gaps tracked**: long symlinks, append-to-file, xattr, mknod, multi-group, hash-tree dirs, inline-data inodes, JBD2 journal, crates.io publication
- **Origin decision**: [`vmisolate` ADR-019](../../../vmisolate/docs/3-design/adr/) reversed the deferral; this repo is the implementation

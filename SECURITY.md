# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| v0 (current) | ✓ |

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report vulnerabilities by email to: phdsystemz@gmail.com

Include:
- A description of the vulnerability
- Steps to reproduce
- The potential impact
- Any suggested fix (optional)

You will receive an acknowledgement within 72 hours. We aim to release a fix within 14 days for confirmed vulnerabilities.

## Scope

justext4 is a filesystem-image builder library and CLI. Security concerns relevant to this project include:

- **Path traversal** in `build-from-host-tree` (host path mapped to image path without escaping)
- **Integer overflow** in block/inode allocation arithmetic
- **Malformed input** in the `spec` decoder (fuzz-tested; see `fuzz/` directory)
- **CLI argument injection** in subcommand flag handling

Out of scope: runtime filesystem mounting, kernel-level vulnerabilities, issues in `e2fsck` or `mkfs.ext4`.

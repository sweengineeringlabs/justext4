#!/usr/bin/env bash
# Regenerate the real-mkfs.ext4 fixture used by the round-trip test.
#
# Run this when:
#   - bumping mke2fs versions (regenerates against newer kernel
#     conventions; integration test re-validates we still parse it)
#   - tweaking the layout (size / label / inode count) we test against
#
# Output: real_minimal.ext4 (committed alongside this script).
#
# Why pre-generated and committed: it locks our integration test to
# bytes a real mke2fs actually produced. A regression in our decoder
# that only surfaces against real-world output is exactly what this
# fixture catches; rebuilding it on every CI run would mask drift in
# mke2fs itself.

set -euo pipefail

cd "$(dirname "$0")"

OUT="real_minimal.ext4"
SIZE_BLOCKS=128
BLOCK_SIZE=4096

# Deterministic flags so the fixture is byte-stable across runs:
#  -F             force (no interactive confirm on small images)
#  -L test        volume label
#  -U <uuid>      pin the volume UUID
#  -E hash_seed=  pin the hash seed for hashed dir tree (defensive)
#  -E nodiscard   don't TRIM
#  -m 0           no reserved blocks for root (smaller, simpler)
#  -N 32          inode count — keep it tiny
#  -b $BLOCK_SIZE block size
#  -t ext4        explicit FS type
#  -O ^has_journal,^huge_file,^64bit,^metadata_csum,^extra_isize
#                 disable optional features so the fixture exercises
#                 our v0 set, not the modern default.

# `dd` to size the empty image first.
TOTAL=$(( SIZE_BLOCKS * BLOCK_SIZE ))
dd if=/dev/zero of="$OUT" bs="$BLOCK_SIZE" count="$SIZE_BLOCKS" status=none

mkfs.ext4 \
    -F \
    -L test \
    -U 11111111-2222-3333-4444-555555555555 \
    -E hash_seed=cafebabe-dead-beef-1234-feedfacecafe,nodiscard \
    -m 0 \
    -N 32 \
    -b "$BLOCK_SIZE" \
    -t ext4 \
    -O '^has_journal,^huge_file,^64bit,^metadata_csum,^extra_isize,^dir_index' \
    "$OUT"

echo "Generated $OUT: $(stat -c%s "$OUT") bytes"
echo "  blocks:     $SIZE_BLOCKS"
echo "  block_size: $BLOCK_SIZE"
echo "  uuid:       11111111-2222-3333-4444-555555555555"
echo "  label:      test"

#!/usr/bin/env bash
# compare_mkfs.sh — time mkfs.ext4 creating empty images of the same
# sizes that the Criterion bench uses. Run on Linux (or WSL2).
#
# Requires: mkfs.ext4 (e2fsprogs), truncate, awk
# Run:   bash scripts/bench/compare_mkfs.sh
#
# Output matches the Criterion bench parameterisation:
#   64-inodes   (36KB metadata written by justext4)
#   512-inodes  (152KB metadata written by justext4)
#   4096-inodes (1070KB metadata written by justext4)

set -euo pipefail

require() { command -v "$1" >/dev/null 2>&1 || { echo "error: $1 not found"; exit 1; }; }
require mkfs.ext4
require truncate
require awk

RUNS=20
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

run_mkfs() {
    local label=$1 inodes=$2 size_kb=$3
    local img="$TMPDIR/image.ext4"
    local total_ns=0

    for _ in $(seq 1 $RUNS); do
        truncate -s "${size_kb}k" "$img"
        local start end
        start=$(date +%s%N)
        mkfs.ext4 -N "$inodes" -q "$img" 2>/dev/null
        end=$(date +%s%N)
        total_ns=$(( total_ns + end - start ))
        rm -f "$img"
    done

    local mean_us=$(( total_ns / RUNS / 1000 ))
    printf "mkfs.ext4  %-14s  N=%-5d  %6d µs  (mean over %d runs)\n" \
        "$label" "$inodes" "$mean_us" "$RUNS"
}

echo "mkfs.ext4 comparison ($(mkfs.ext4 -V 2>&1 | head -1))"
echo "-----------------------------------------------------------"
run_mkfs "64-inodes"   64   1024
run_mkfs "512-inodes"  512  8192
run_mkfs "4096-inodes" 4096 65536
echo ""
echo "Compare against: cargo bench -p swe_justext4_ext4 --bench write_image"

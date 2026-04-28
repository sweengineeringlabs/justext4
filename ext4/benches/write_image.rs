//! Criterion benchmarks for `ext4::format`.
//!
//! Measures: time to produce a valid empty ext4 image in memory,
//! parameterised by inode count (which drives inode-table size and
//! therefore the amount of bytes written).
//!
//! Run:
//!   cargo bench -p swe_justext4_ext4 --bench write_image
//!
//! Market comparison:
//!   scripts/bench/compare_mkfs.sh   (requires mkfs.ext4 on PATH)

use std::io::Cursor;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use ext4::{Config, format};

/// Returns the number of bytes `format` will actually write for the
/// given config: one block for each of (superblock-block, GDT, block
/// bitmap, inode bitmap) + inode_table_blocks + root_dir_block.
fn bytes_written(block_size: u32, inodes_per_group: u32) -> u64 {
    let inode_size = 256u64;
    let bs = block_size as u64;
    let inode_table_blocks = ((inodes_per_group as u64) * inode_size).div_ceil(bs);
    let root_dir_block = 4 + inode_table_blocks;
    (root_dir_block + 1) * bs
}

fn bench_format_image(c: &mut Criterion) {
    let mut group = c.benchmark_group("format_image");

    // (inodes_per_group, size_blocks, label)
    // size_blocks must exceed metadata_blocks; we choose 4× as headroom.
    let cases: &[(u32, u32, &str)] = &[
        (64,   256,   "64-inodes"),
        (512,  2048,  "512-inodes"),
        (4096, 16384, "4096-inodes"),
    ];

    for &(inodes, size_blocks, label) in cases {
        let written = bytes_written(4096, inodes);
        group.throughput(Throughput::Bytes(written));
        group.bench_with_input(
            BenchmarkId::new("justext4", label),
            &(inodes, size_blocks),
            |b, &(inodes_per_group, blocks)| {
                b.iter(|| {
                    let mut buf = Cursor::new(Vec::with_capacity(written as usize));
                    let config = Config {
                        block_size: 4096,
                        size_blocks: blocks,
                        inodes_per_group,
                        volume_label: b"bench".to_vec(),
                    };
                    format(&mut buf, &config).expect("format must succeed");
                    buf.into_inner()
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_format_image);
criterion_main!(benches);

#![no_main]
//! Fuzz target: `Inode::decode` must surface a typed error or a
//! successful decode for any byte slice — never panic. The
//! superblock is fixed (synthesized once via `Superblock::decode`
//! over a minimal valid buffer) so the fuzzer mutates only the
//! inode bytes, not the SB shape.

use libfuzzer_sys::fuzz_target;
use spec::{Inode, Superblock, EXT4_MAGIC, SUPERBLOCK_SIZE};
use std::sync::OnceLock;

static SB: OnceLock<Superblock> = OnceLock::new();

fn minimal_superblock() -> &'static Superblock {
    SB.get_or_init(|| {
        // Mirror the pattern in `spec::inode::tests::make_sb_with_inode_size`.
        // 4 KiB blocks, rev_level = 1, inode_size = 256.
        let mut buf = vec![0u8; SUPERBLOCK_SIZE];
        buf[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes());
        buf[0x38..0x3A].copy_from_slice(&EXT4_MAGIC.to_le_bytes());
        buf[0x4C..0x50].copy_from_slice(&1u32.to_le_bytes());
        buf[0x58..0x5A].copy_from_slice(&256u16.to_le_bytes());
        Superblock::decode(&buf).expect("fuzz harness SB must decode")
    })
}

fuzz_target!(|data: &[u8]| {
    let sb = minimal_superblock();
    let _ = Inode::decode(data, sb);
});

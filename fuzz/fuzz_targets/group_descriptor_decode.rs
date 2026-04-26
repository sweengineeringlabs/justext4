#![no_main]
//! Fuzz target: `GroupDescriptor::decode` for any byte slice must
//! return a typed error or a valid descriptor — never panic. The
//! superblock is fixed so the fuzzer mutates only the GDT entry
//! bytes; we hold it in a `OnceLock` to avoid re-decoding per call.

use libfuzzer_sys::fuzz_target;
use spec::{GroupDescriptor, Superblock, EXT4_MAGIC, SUPERBLOCK_SIZE};
use std::sync::OnceLock;

static SB: OnceLock<Superblock> = OnceLock::new();

fn minimal_superblock() -> &'static Superblock {
    SB.get_or_init(|| {
        // 4 KiB blocks, rev 1, inode_size = 256, no 64BIT incompat
        // → group_descriptor_size() = 32.
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
    let _ = GroupDescriptor::decode(data, sb);
});

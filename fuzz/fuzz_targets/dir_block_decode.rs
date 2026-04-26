#![no_main]
//! Fuzz target: `decode_dir_block` walks a variable-length record
//! stream until the buffer is exhausted. Any byte slice must
//! resolve to either a `Vec<DirEntry>` or a typed error — no panic.

use libfuzzer_sys::fuzz_target;
use spec::decode_dir_block;

fuzz_target!(|data: &[u8]| {
    let _ = decode_dir_block(data);
});

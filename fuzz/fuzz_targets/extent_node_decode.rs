#![no_main]
//! Fuzz target: `decode_extent_node` reads a 12-byte header then a
//! variable count of 12-byte entries. Any byte slice must resolve
//! to either an `ExtentNode` or a typed error — no panic.

use libfuzzer_sys::fuzz_target;
use spec::decode_extent_node;

fuzz_target!(|data: &[u8]| {
    let _ = decode_extent_node(data);
});

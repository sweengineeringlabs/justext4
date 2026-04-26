#![no_main]
//! Fuzz target: `Superblock::decode` must surface a typed error or
//! a successful decode for any byte slice — never panic.

use libfuzzer_sys::fuzz_target;
use spec::Superblock;

fuzz_target!(|data: &[u8]| {
    let _ = Superblock::decode(data);
});

//! Bit-level helpers for ext4 block and inode bitmaps.
//!
//! ext4 uses little-endian bit numbering within a byte: bit `N`
//! lives at byte `N/8`, mask `1 << (N % 8)`. The kernel's
//! `ext4_test_bit` family applies this convention; these helpers
//! mirror it byte-for-byte so allocator output round-trips
//! through the kernel's view of the same bitmap.
//!
//! Pure functions over `&[u8]` / `&mut [u8]` — no allocation, no
//! IO, callable from both spec-layer encoders and ext4-layer
//! allocators.

/// Test the bit at index `bit`. Returns `false` for indices past
/// the buffer end (treated as the conceptual extension of the
/// bitmap with all-clear bits).
pub fn get_bit(buf: &[u8], bit: usize) -> bool {
    let byte = bit / 8;
    if byte >= buf.len() {
        return false;
    }
    let mask = 1u8 << (bit % 8);
    buf[byte] & mask != 0
}

/// Set the bit at index `bit`. Panics on out-of-bounds — callers
/// must respect the bitmap's bit count (typically `block_size *
/// 8`, or `inodes_per_group` for inode bitmaps where the bitmap
/// block can be larger than the meaningful range).
pub fn set_bit(buf: &mut [u8], bit: usize) {
    let byte = bit / 8;
    let mask = 1u8 << (bit % 8);
    buf[byte] |= mask;
}

/// Clear the bit at index `bit`. Panics on out-of-bounds.
pub fn clear_bit(buf: &mut [u8], bit: usize) {
    let byte = bit / 8;
    let mask = 1u8 << (bit % 8);
    buf[byte] &= !mask;
}

/// Find the lowest-indexed clear bit in `buf` whose index is
/// strictly less than `max_bit`. Returns `None` if every bit
/// below `max_bit` is set.
///
/// The full-byte fast-path (`0xFF` skip) keeps allocator hot
/// paths cheap on largely-allocated bitmaps.
pub fn find_first_zero(buf: &[u8], max_bit: usize) -> Option<usize> {
    let max_byte = max_bit.div_ceil(8).min(buf.len());
    for (byte_idx, &byte) in buf.iter().enumerate().take(max_byte) {
        if byte == 0xFF {
            continue;
        }
        for bit_in_byte in 0..8 {
            let bit = byte_idx * 8 + bit_in_byte;
            if bit >= max_bit {
                return None;
            }
            if byte & (1 << bit_in_byte) == 0 {
                return Some(bit);
            }
        }
    }
    None
}

/// Find the lowest-indexed run of `n` consecutive clear bits in
/// `buf`. Returns the starting bit index, or `None` if no such
/// run exists below `max_bit`.
///
/// `n = 0` returns `Some(0)` (vacuously satisfied — the empty run
/// fits anywhere); the contract preserves arithmetic predictability
/// for callers that compute `n` from a file size.
pub fn find_first_zero_run(buf: &[u8], n: usize, max_bit: usize) -> Option<usize> {
    if n == 0 {
        return Some(0);
    }
    let mut run_start: Option<usize> = None;
    let mut run_len = 0usize;
    for bit in 0..max_bit {
        if !get_bit(buf, bit) {
            if run_start.is_none() {
                run_start = Some(bit);
            }
            run_len += 1;
            if run_len == n {
                return run_start;
            }
        } else {
            run_start = None;
            run_len = 0;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `get_bit` / `set_bit` / `clear_bit` round-trip cleanly.
    #[test]
    fn test_bit_round_trip() {
        let mut buf = vec![0u8; 4];
        assert!(!get_bit(&buf, 5));
        set_bit(&mut buf, 5);
        assert!(get_bit(&buf, 5));
        clear_bit(&mut buf, 5);
        assert!(!get_bit(&buf, 5));
    }

    /// Bit numbering is little-endian within a byte: bit 0 of
    /// byte 1 lives at the LSB of `buf[1]`.
    ///
    /// Bug it catches: a parser using big-endian bit ordering
    /// (bit 0 = MSB) would produce bitmaps the kernel can't
    /// interpret. Allocators built on the wrong convention would
    /// fight the kernel for every block the FS actually uses.
    #[test]
    fn test_bit_little_endian_within_byte() {
        let mut buf = vec![0u8; 2];
        set_bit(&mut buf, 0); // LSB of byte 0
        assert_eq!(buf[0], 0b0000_0001);
        set_bit(&mut buf, 7); // MSB of byte 0
        assert_eq!(buf[0], 0b1000_0001);
        set_bit(&mut buf, 8); // LSB of byte 1
        assert_eq!(buf[1], 0b0000_0001);
    }

    /// `get_bit` past the buffer end returns `false` rather than
    /// panicking.
    ///
    /// Bug it catches: many allocator hot paths probe one bit
    /// past the meaningful range to detect "no more space"; if
    /// `get_bit` panicked there, every successful allocation in
    /// a full bitmap would crash on the post-allocation
    /// "anything left?" check.
    #[test]
    fn test_get_bit_out_of_bounds_returns_false() {
        let buf = vec![0xFFu8; 2];
        assert!(!get_bit(&buf, 100));
    }

    /// `find_first_zero` on an all-zero buffer returns 0.
    #[test]
    fn test_find_first_zero_in_clean_buffer_returns_zero() {
        let buf = vec![0u8; 4];
        assert_eq!(find_first_zero(&buf, 32), Some(0));
    }

    /// `find_first_zero` skips set bits and returns the first
    /// clear one.
    #[test]
    fn test_find_first_zero_skips_set_bits() {
        let mut buf = vec![0u8; 4];
        set_bit(&mut buf, 0);
        set_bit(&mut buf, 1);
        set_bit(&mut buf, 2);
        assert_eq!(find_first_zero(&buf, 32), Some(3));
    }

    /// `find_first_zero` returns `None` when every bit below
    /// `max_bit` is set.
    ///
    /// Bug it catches: a search that returns `Some(0)` on a full
    /// bitmap (because `0xFF` was misinterpreted) would cause
    /// double-allocation of bit 0 — a corruption-class bug.
    #[test]
    fn test_find_first_zero_all_full_returns_none() {
        let buf = vec![0xFFu8; 4];
        assert_eq!(find_first_zero(&buf, 32), None);
    }

    /// `find_first_zero` honours `max_bit` even when the buffer
    /// has clear bits past it.
    ///
    /// Bug it catches: inode bitmaps live in a full-block buffer
    /// (typically 4 KiB = 32768 bits) but `inodes_per_group` is
    /// often much smaller (8192 on modern images). Returning a
    /// bit past `inodes_per_group` would allocate inode N where
    /// the inode table doesn't extend to N — every read of that
    /// inode would land in adjacent metadata.
    #[test]
    fn test_find_first_zero_respects_max_bit_below_buffer_size() {
        let mut buf = vec![0u8; 4];
        // First 16 bits all set; bits 16+ all clear.
        buf[0] = 0xFF;
        buf[1] = 0xFF;
        // max_bit = 16 → no clear bits below it.
        assert_eq!(find_first_zero(&buf, 16), None);
        // max_bit = 24 → bit 16 is clear.
        assert_eq!(find_first_zero(&buf, 24), Some(16));
    }

    /// `find_first_zero_run` on a clean buffer returns 0 for any
    /// run length.
    #[test]
    fn test_find_first_zero_run_in_clean_buffer_returns_zero() {
        let buf = vec![0u8; 8];
        assert_eq!(find_first_zero_run(&buf, 5, 64), Some(0));
        assert_eq!(find_first_zero_run(&buf, 1, 64), Some(0));
    }

    /// `find_first_zero_run` resets the running tally when it
    /// hits a set bit.
    ///
    /// Bug it catches: a run-finder that doesn't reset on a set
    /// bit (e.g. counts total clear bits, not consecutive)
    /// would return a "run" that's actually fragmented across
    /// the bitmap. Allocating data blocks in such a "run" would
    /// step into the in-use region of the disk, corrupting the
    /// existing tenant.
    #[test]
    fn test_find_first_zero_run_skips_past_obstruction() {
        let mut buf = vec![0u8; 8];
        // bits 0-2 clear, bit 3 set, bits 4+ clear → first
        // 5-bit run starts at bit 4.
        set_bit(&mut buf, 3);
        assert_eq!(find_first_zero_run(&buf, 5, 64), Some(4));
    }

    /// `find_first_zero_run(0)` is vacuously satisfied at bit 0.
    ///
    /// Bug it catches: a higher-level allocator computing
    /// `n_blocks = data.len().div_ceil(block_size)` for an
    /// empty file gets `n = 0`. The contract preserves
    /// arithmetic predictability — the empty run fits anywhere.
    #[test]
    fn test_find_first_zero_run_zero_length_returns_zero() {
        let buf = vec![0xFFu8; 4];
        assert_eq!(find_first_zero_run(&buf, 0, 32), Some(0));
    }

    /// `find_first_zero_run` returns None when no contiguous
    /// run of the requested length exists.
    #[test]
    fn test_find_first_zero_run_no_room_returns_none() {
        let mut buf = vec![0u8; 4];
        // Set every other bit so max contiguous run is 1.
        for bit in (0..32).step_by(2) {
            set_bit(&mut buf, bit);
        }
        assert_eq!(find_first_zero_run(&buf, 2, 32), None);
    }
}

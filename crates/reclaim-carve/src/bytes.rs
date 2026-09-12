//! Bounds-checked integer readers for on-disk byte parsing.
//!
//! Every accessor returns `Option`; none panic on short/malformed input
//! (build guide Part 1.4 rule 3).

/// Read a big-endian `u16` at `off`.
#[must_use]
pub fn be_u16(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off.checked_add(2)?)?;
    Some(u16::from_be_bytes([*s.first()?, *s.get(1)?]))
}

/// Read a little-endian `u16` at `off`.
#[must_use]
pub fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

/// Read a big-endian `u32` at `off`.
#[must_use]
pub fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_be_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

/// Read a little-endian `u32` at `off`.
#[must_use]
pub fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

/// Read a big-endian `u64` at `off`.
#[must_use]
pub fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    let mut a = [0u8; 8];
    for (d, x) in a.iter_mut().zip(s.iter()) {
        *d = *x;
    }
    Some(u64::from_be_bytes(a))
}

/// Read a little-endian `u64` at `off`.
#[must_use]
pub fn le_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    let mut a = [0u8; 8];
    for (d, x) in a.iter_mut().zip(s.iter()) {
        *d = *x;
    }
    Some(u64::from_le_bytes(a))
}

/// A fixed-length byte tag at `off`.
#[must_use]
pub fn tag(b: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    b.get(off..off.checked_add(len)?)
}

/// True if `b` at `off` equals `needle`.
#[must_use]
pub fn eq_at(b: &[u8], off: usize, needle: &[u8]) -> bool {
    match b.get(off..off.saturating_add(needle.len())) {
        Some(s) => s == needle,
        None => false,
    }
}

/// Greatest common divisor (for block-size inference).
#[must_use]
pub fn gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads() {
        let b = [0x12, 0x34, 0x56, 0x78, 0x9a];
        assert_eq!(be_u16(&b, 0), Some(0x1234));
        assert_eq!(le_u16(&b, 0), Some(0x3412));
        assert_eq!(be_u32(&b, 0), Some(0x1234_5678));
        assert_eq!(le_u32(&b, 1), Some(0x9a78_5634));
        assert_eq!(be_u32(&b, 3), None);
        assert!(eq_at(&b, 1, &[0x34, 0x56]));
        assert!(!eq_at(&b, 4, &[0x9a, 0x00]));
    }

    #[test]
    fn gcd_basics() {
        assert_eq!(gcd(0, 5), 5);
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(131072, 262144), 131072);
    }
}

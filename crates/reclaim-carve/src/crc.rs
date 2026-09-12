//! IEEE CRC-32 (used by PNG chunk validation and EnCase later).

use std::sync::OnceLock;

fn table() -> &'static [u32; 256] {
    static T: OnceLock<[u32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 256];
        let mut n = 0usize;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            if let Some(slot) = t.get_mut(n) {
                *slot = c;
            }
            n += 1;
        }
        t
    })
}

/// CRC-32 (IEEE, as used by zlib/PNG) of `data`.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let t = table();
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = ((c ^ u32::from(b)) & 0xFF) as usize;
        let e = t.get(idx).copied().unwrap_or(0);
        c = e ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // CRC-32 of "123456789" is 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}

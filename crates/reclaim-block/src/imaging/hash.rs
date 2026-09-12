//! Incremental digest over the whole image and per chunk (BLAKE3 or SHA-256).

use crate::imaging::map::HashAlgo;
use sha2::{Digest as _, Sha256};

/// A streaming hasher selectable at runtime.
pub enum Digest {
    /// BLAKE3.
    Blake3(Box<blake3::Hasher>),
    /// SHA-256.
    Sha256(Box<Sha256>),
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Digest::Blake3(_) => "Digest::Blake3",
            Digest::Sha256(_) => "Digest::Sha256",
        })
    }
}

impl Digest {
    /// New hasher for `algo`.
    #[must_use]
    pub fn new(algo: HashAlgo) -> Self {
        match algo {
            HashAlgo::Blake3 => Digest::Blake3(Box::new(blake3::Hasher::new())),
            HashAlgo::Sha256 => Digest::Sha256(Box::new(Sha256::new())),
        }
    }

    /// Feed bytes.
    pub fn update(&mut self, data: &[u8]) {
        match self {
            Digest::Blake3(h) => {
                h.update(data);
            }
            Digest::Sha256(h) => h.update(data),
        }
    }

    /// Finalize to a lowercase hex string.
    #[must_use]
    pub fn finalize_hex(self) -> String {
        match self {
            Digest::Blake3(h) => h.finalize().to_hex().to_string(),
            Digest::Sha256(h) => {
                let out = h.finalize();
                let mut s = String::with_capacity(out.len() * 2);
                for b in out {
                    s.push_str(&format!("{b:02x}"));
                }
                s
            }
        }
    }
}

/// One-shot hash of `data`.
#[must_use]
pub fn hash_bytes(algo: HashAlgo, data: &[u8]) -> String {
    let mut d = Digest::new(algo);
    d.update(data);
    d.finalize_hex()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known() {
        // SHA-256("abc")
        assert_eq!(
            hash_bytes(HashAlgo::Sha256, b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn blake3_stable() {
        let a = hash_bytes(HashAlgo::Blake3, b"hello");
        let b = hash_bytes(HashAlgo::Blake3, b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}

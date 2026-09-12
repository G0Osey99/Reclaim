//! A tiny fixed-length bitset backed by `u64` words.
//!
//! Used for per-sector good/bad/unread status in [`crate::ReadResult`]. A read
//! of 16 MiB at 512-byte sectors spans 32768 sectors, so a `Vec<bool>` would be
//! wasteful; one bit per sector keeps it compact. Implemented in-house to avoid
//! a dependency and to stay within the crate's no-indexing lint.

/// Fixed-length set of bits, all clear on construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitset {
    bits: usize,
    words: Vec<u64>,
}

impl Bitset {
    /// Create a bitset of `bits` bits, all clear.
    #[must_use]
    pub fn new(bits: usize) -> Self {
        let words = bits.div_ceil(64);
        Bitset {
            bits,
            words: vec![0u64; words],
        }
    }

    /// Number of bits.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bits
    }

    /// True if there are no bits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Set bit `i` (no-op if out of range).
    pub fn set(&mut self, i: usize) {
        if i >= self.bits {
            return;
        }
        if let Some(word) = self.words.get_mut(i / 64) {
            *word |= 1u64 << (i % 64);
        }
    }

    /// Get bit `i` (false if out of range).
    #[must_use]
    pub fn get(&self, i: usize) -> bool {
        if i >= self.bits {
            return false;
        }
        self.words
            .get(i / 64)
            .is_some_and(|word| (*word & (1u64 << (i % 64))) != 0)
    }

    /// True if any bit is set.
    #[must_use]
    pub fn any(&self) -> bool {
        self.words.iter().any(|w| *w != 0)
    }

    /// Number of set bits.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Iterate the indices of set bits, ascending.
    pub fn iter_set(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.bits).filter(move |&i| self.get(i))
    }
}

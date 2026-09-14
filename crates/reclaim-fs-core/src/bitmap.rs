//! [`Bitmap`] — a filesystem allocation bitmap mapped back to source bytes.
//!
//! Each bit represents one cluster of `block_size` bytes; bit 0 covers the
//! cluster that begins at `base_offset` (the cluster-heap start, volume-relative
//! as the engine sees it). `1` = allocated/in-use. The carver consults this to
//! scan unallocated space only, and the recoverability score uses it to tell
//! whether a deleted file's blocks have been reused (overwritten).

/// A cluster allocation bitmap with enough geometry to translate a byte offset
/// into an allocation state.
#[derive(Clone, Debug)]
pub struct Bitmap {
    /// Bytes per cluster (the granularity of one bit).
    block_size: u32,
    /// Byte offset that bit 0 covers.
    base_offset: u64,
    /// Number of meaningful bits (clusters).
    block_count: u64,
    /// Packed bits, LSB-first within each byte.
    bits: Vec<u8>,
}

impl Bitmap {
    /// Build a bitmap from packed bytes (LSB-first). `block_count` is the number
    /// of meaningful clusters; trailing bits in the last byte are ignored.
    #[must_use]
    pub fn new(block_size: u32, base_offset: u64, block_count: u64, bits: Vec<u8>) -> Self {
        Bitmap {
            block_size: block_size.max(1),
            base_offset,
            block_count,
            bits,
        }
    }

    /// Bytes per cluster.
    #[must_use]
    pub fn block_size(&self) -> u32 {
        self.block_size
    }

    /// Number of clusters the bitmap describes.
    #[must_use]
    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    /// Allocation state of cluster `index` (out of range ⇒ `true`, i.e. treated
    /// as allocated/unknown so the carver does not wander off the heap).
    #[must_use]
    pub fn is_block_allocated(&self, index: u64) -> bool {
        if index >= self.block_count {
            return true;
        }
        let byte = (index / 8) as usize;
        let bit = (index % 8) as u8;
        match self.bits.get(byte) {
            Some(b) => (b >> bit) & 1 == 1,
            None => true,
        }
    }

    /// Allocation state at a volume-relative byte `offset`. Offsets before the
    /// cluster heap (boot sector, FAT, bitmap itself) count as allocated.
    #[must_use]
    pub fn is_offset_allocated(&self, offset: u64) -> bool {
        if offset < self.base_offset {
            return true;
        }
        let index = (offset - self.base_offset) / u64::from(self.block_size);
        self.is_block_allocated(index)
    }

    /// Fraction (0..1) of `[offset, offset+len)` that is currently *unallocated*
    /// (free). Sampled per cluster. A deleted file sitting entirely in free
    /// space returns ~1.0; one whose blocks have been reused returns lower.
    #[must_use]
    pub fn unallocated_fraction(&self, offset: u64, len: u64) -> f32 {
        if len == 0 {
            return 1.0;
        }
        let bs = u64::from(self.block_size);
        let first = offset / bs;
        let last = offset.saturating_add(len.saturating_sub(1)) / bs;
        let mut total: u64 = 0;
        let mut free: u64 = 0;
        // Sample one point per cluster; cap the span so a crafted huge size
        // can't stall the score.
        let mut c = first;
        while c <= last && total < 1_000_000 {
            let byte_off = c.saturating_mul(bs);
            total += 1;
            if !self.is_offset_allocated(byte_off) {
                free += 1;
            }
            c += 1;
        }
        if total == 0 {
            return 1.0;
        }
        free as f32 / total as f32
    }

    /// Count of allocated clusters (for diagnostics).
    #[must_use]
    pub fn allocated_count(&self) -> u64 {
        // Bounded by the bits actually present (a crafted `block_count` must
        // not drive a multi-billion-iteration loop); popcount per byte.
        let n = self
            .block_count
            .min((self.bits.len() as u64).saturating_mul(8));
        let full = (n / 8) as usize;
        let rem = (n % 8) as u32;
        let mut count: u64 = self
            .bits
            .get(..full)
            .map(|b| b.iter().map(|x| u64::from(x.count_ones())).sum())
            .unwrap_or(0);
        if rem > 0 {
            if let Some(last) = self.bits.get(full) {
                let mask = (1u16 << rem) as u8 - 1;
                count += u64::from((last & mask).count_ones());
            }
        }
        count
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn bit_addressing() {
        // 16 clusters of 4096 bytes, heap starts at 0x10000.
        // Clusters 0,1,2 allocated; 3..16 free.
        let bits = vec![0b0000_0111u8, 0b0000_0000u8];
        let bm = Bitmap::new(4096, 0x10000, 16, bits);
        assert!(bm.is_block_allocated(0));
        assert!(bm.is_block_allocated(2));
        assert!(!bm.is_block_allocated(3));
        // Offset inside cluster 0.
        assert!(bm.is_offset_allocated(0x10000));
        assert!(bm.is_offset_allocated(0x10000 + 4095));
        // Offset inside cluster 3 (free).
        assert!(!bm.is_offset_allocated(0x10000 + 3 * 4096));
        // Pre-heap offset is allocated.
        assert!(bm.is_offset_allocated(0));
        // Out-of-range index is "allocated" (conservative).
        assert!(bm.is_block_allocated(99));
    }

    #[test]
    fn unalloc_fraction() {
        let bits = vec![0b0000_0111u8, 0b0000_0000u8];
        let bm = Bitmap::new(4096, 0x10000, 16, bits);
        // Clusters 3,4 (both free).
        let f = bm.unallocated_fraction(0x10000 + 3 * 4096, 2 * 4096);
        assert!((f - 1.0).abs() < 1e-6, "got {f}");
        // Clusters 0,1 (both allocated).
        let f2 = bm.unallocated_fraction(0x10000, 2 * 4096);
        assert!(f2.abs() < 1e-6, "got {f2}");
    }
}

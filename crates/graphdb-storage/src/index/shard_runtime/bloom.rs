use std::hash::{Hash, Hasher};

/// Simple bloom filter for range-scan skip optimization.
/// Uses a fixed-size bit array with 3 hash functions.
pub(crate) struct RangeBloom {
    bits: bitvec::vec::BitVec,
    seeds: [u64; 3],
}

impl RangeBloom {
    pub(crate) fn new() -> Self {
        // 65536 bits = 8 KB per filter, handles ~5000 entries with ~1% FP rate
        Self {
            bits: bitvec::vec::BitVec::repeat(false, 65536),
            seeds: [0x1234, 0x5678, 0x9abc],
        }
    }

    fn hash_index(&self, key: &[u8], seed: u64) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        key.hash(&mut hasher);
        hasher.finish() as usize % self.bits.len()
    }

    pub(crate) fn insert(&mut self, key: &[u8]) {
        for seed in &self.seeds {
            let idx = self.hash_index(key, *seed);
            self.bits.set(idx, true);
        }
    }

    pub(crate) fn might_contain(&self, key: &[u8]) -> bool {
        for seed in &self.seeds {
            let idx = self.hash_index(key, *seed);
            if !self.bits[idx] {
                return false;
            }
        }
        true
    }
}

use std::hash::{Hash, Hasher};

/// Simple bloom filter for range-scan skip optimization.
/// Uses a fixed-size bit array with 3 hash functions.
///
/// Query and hit counters back the hit-rate export and the saturation
/// bypass on the range pre-check path: a filter that has proven unable to
/// filter anything stops charging queries its hashes.
pub(crate) struct RangeBloom {
    bits: bitvec::vec::BitVec,
    seeds: [u64; 3],
    queries: u64,
    hits: u64,
}

/// Queries after which a never-filtering bloom is bypassed.
const SATURATION_QUERIES: u64 = 1024;

impl RangeBloom {
    pub(crate) fn new() -> Self {
        // 65536 bits = 8 KB per filter, handles ~5000 entries with ~1% FP rate
        Self {
            bits: bitvec::vec::BitVec::repeat(false, 65536),
            seeds: [0x1234, 0x5678, 0x9abc],
            queries: 0,
            hits: 0,
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

    pub(crate) fn might_contain(&mut self, key: &[u8]) -> bool {
        self.queries = self.queries.saturating_add(1);
        for seed in &self.seeds {
            let idx = self.hash_index(key, *seed);
            if !self.bits[idx] {
                return false;
            }
        }
        self.hits = self.hits.saturating_add(1);
        true
    }

    pub(crate) fn queries(&self) -> u64 {
        self.queries
    }

    pub(crate) fn hits(&self) -> u64 {
        self.hits
    }

    /// Share of queries answered positive, 0.0 when never queried.
    pub(crate) fn hit_rate(&self) -> f64 {
        if self.queries == 0 {
            0.0
        } else {
            self.hits as f64 / self.queries as f64
        }
    }

    /// Whether the filter has proven unable to filter anything: enough
    /// queries served with (almost) every one positive. Callers bypass the
    /// pre-check then, keeping result semantics identical while saving the
    /// hashes on a filter that never skips.
    pub(crate) fn is_saturated(&self) -> bool {
        self.queries >= SATURATION_QUERIES && self.hit_rate() >= 0.99
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rate_tracks_queries_and_hits() {
        let mut bloom = RangeBloom::new();
        assert_eq!(bloom.hit_rate(), 0.0);
        assert!(!bloom.is_saturated());
        bloom.insert(b"alpha");
        assert!(bloom.might_contain(b"alpha"));
        assert!(!bloom.might_contain(b"definitely-absent-key-xyz"));
        assert!((bloom.hit_rate() - 0.5).abs() < 1e-9);
        assert!(!bloom.is_saturated());
    }

    #[test]
    fn always_positive_filter_saturates() {
        let mut bloom = RangeBloom::new();
        bloom.insert(b"alpha");
        for _ in 0..SATURATION_QUERIES {
            assert!(bloom.might_contain(b"alpha"));
        }
        assert!(bloom.is_saturated());
    }
}

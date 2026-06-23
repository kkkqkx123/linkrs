//! Simple Bloom Filter for CSR edge deletion optimization
//!
//! Uses a bit vector to quickly check if an edge ID might be in the deletion set.
//! False positives are possible (will check the actual deletion set), but false negatives are not.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Simple Bloom Filter for edge deletion detection
///
/// Provides O(1) probabilistic membership testing for edge IDs.
/// Reduces unnecessary lookups in the tombstone map during time-travel queries.
#[derive(Debug, Clone)]
pub struct EdgeDeletionBloomFilter {
    bits: Vec<u64>,
    bit_count: usize,
    hash_functions: usize,
}

impl EdgeDeletionBloomFilter {
    /// Create a new bloom filter with desired capacity
    ///
    /// Allocates space for approximately `capacity` deletions
    /// with 1% false positive probability.
    pub fn with_capacity(capacity: usize) -> Self {
        // For 1% FP rate: bit_count = -1.44 * n * log2(p)
        // With p=0.01: bit_count ≈ 9.6 * n
        let bit_count = ((capacity as f64 * 9.6) as usize).max(8);
        let word_count = (bit_count + 63) / 64;

        // Number of hash functions: k = (bit_count / capacity) * ln(2)
        let hash_functions = (((bit_count as f64 / capacity.max(1) as f64) * 0.693) as usize).max(1);

        Self {
            bits: vec![0u64; word_count],
            bit_count,
            hash_functions,
        }
    }

    /// Insert an element (edge ID that was deleted)
    pub fn insert(&mut self, edge_id: u64) {
        for i in 0..self.hash_functions {
            let bit_pos = self.hash_position(edge_id, i);
            let word_idx = bit_pos / 64;
            let bit_idx = bit_pos % 64;

            if word_idx < self.bits.len() {
                self.bits[word_idx] |= 1u64 << bit_idx;
            }
        }
    }

    /// Check if an element might be in the filter
    ///
    /// Returns true if definitely not in the set (can skip tombstone check)
    /// Returns false if might be in the set (must check tombstone map)
    pub fn might_contain(&self, edge_id: u64) -> bool {
        for i in 0..self.hash_functions {
            let bit_pos = self.hash_position(edge_id, i);
            let word_idx = bit_pos / 64;
            let bit_idx = bit_pos % 64;

            if word_idx >= self.bits.len() {
                return false; // Uninitialized memory = not in set
            }

            let has_bit = (self.bits[word_idx] & (1u64 << bit_idx)) != 0;
            if !has_bit {
                return false; // At least one bit is 0, definitely not in set
            }
        }
        true // All bits are 1, might be in set
    }

    /// Get memory usage in bytes
    pub fn memory_bytes(&self) -> usize {
        self.bits.len() * 8
    }

    /// Clear all bits
    pub fn clear(&mut self) {
        self.bits.iter_mut().for_each(|w| *w = 0);
    }

    /// Hash position for element using i-th hash function
    fn hash_position(&self, element: u64, i: usize) -> usize {
        let mut hasher = DefaultHasher::new();
        element.hash(&mut hasher);
        (i as u64).hash(&mut hasher);
        (hasher.finish() as usize) % self.bit_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic() {
        let mut filter = EdgeDeletionBloomFilter::with_capacity(100);

        // Insert some elements
        for i in 0..100 {
            filter.insert(i);
        }

        // Check inserted elements
        for i in 0..100 {
            assert!(filter.might_contain(i), "Element {} should be in filter", i);
        }

        // Check non-inserted elements (may have false positives)
        let mut false_positives = 0;
        for i in 100..200 {
            if filter.might_contain(i) {
                false_positives += 1;
            }
        }

        // Should have reasonable false positive rate (~1%)
        assert!(false_positives < 10, "Too many false positives: {}", false_positives);
    }

    #[test]
    fn test_bloom_filter_memory() {
        let filter = EdgeDeletionBloomFilter::with_capacity(1000);
        let mem = filter.memory_bytes();

        // Should be around 1.2KB for 1000 elements
        assert!(mem > 1000 && mem < 2000, "Memory usage: {} bytes", mem);
    }

    #[test]
    fn test_bloom_filter_clear() {
        let mut filter = EdgeDeletionBloomFilter::with_capacity(100);
        filter.insert(42);

        assert!(filter.might_contain(42));

        filter.clear();
        assert!(!filter.might_contain(42));
    }
}

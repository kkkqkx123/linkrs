//! Bloom Filter
//!
//! A space-efficient probabilistic data structure for set membership testing.
//! Provides O(k) time complexity where k is the number of hash functions.
//!
//! # Features
//!
//! - Configurable false positive rate
//! - Automatic optimal parameter calculation
//! - Memory-efficient bit storage using BitVec
//! - Support for any byte-slice key

use bitvec::prelude::*;

#[derive(Debug, Clone)]
pub struct BloomFilter {
    bitmap: BitVec<u8, Lsb0>,
    hash_count: usize,
    bit_count: usize,
    item_count: usize,
}

impl BloomFilter {
    pub fn new(expected_items: usize, false_positive_rate: f64) -> Self {
        let ln2 = std::f64::consts::LN_2;

        let bit_count = if expected_items == 0 {
            1024
        } else {
            let m = (-(expected_items as f64) * false_positive_rate.ln() / (ln2 * ln2)).ceil();
            m.max(64.0) as usize
        };

        let hash_count = if expected_items == 0 {
            3
        } else {
            let k = (bit_count as f64 / expected_items as f64 * ln2).ceil();
            k.clamp(1.0, 20.0) as usize
        };

        Self {
            bitmap: BitVec::repeat(false, bit_count),
            hash_count,
            bit_count,
            item_count: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(capacity, 0.01)
    }

    pub fn insert(&mut self, key: &[u8]) {
        let hashes = self.hash_key(key);
        for h in hashes {
            let idx = h % self.bit_count;
            self.bitmap.set(idx, true);
        }
        self.item_count += 1;
    }

    pub fn insert_str(&mut self, key: &str) {
        self.insert(key.as_bytes());
    }

    pub fn insert_u64(&mut self, key: u64) {
        self.insert(&key.to_le_bytes());
    }

    pub fn might_contain(&self, key: &[u8]) -> bool {
        let hashes = self.hash_key(key);
        hashes.iter().all(|&h| {
            let idx = h % self.bit_count;
            self.bitmap[idx]
        })
    }

    pub fn might_contain_str(&self, key: &str) -> bool {
        self.might_contain(key.as_bytes())
    }

    pub fn might_contain_u64(&self, key: u64) -> bool {
        self.might_contain(&key.to_le_bytes())
    }

    fn hash_key(&self, key: &[u8]) -> Vec<usize> {
        let h1 = Self::murmur_hash3(key, 0);
        let h2 = Self::murmur_hash3(key, h1 as u32);

        (0..self.hash_count)
            .map(|i| {
                let combined = h1.wrapping_add((i as u64).wrapping_mul(h2));
                combined as usize
            })
            .collect()
    }

    fn murmur_hash3(data: &[u8], seed: u32) -> u64 {
        let c1: u64 = 0x87c37b91114253d5;
        let c2: u64 = 0x4cf5ad432745937f;

        let mut h: u64 = seed as u64;
        let mut processed = 0;

        let chunks = data.len() / 8;
        for i in 0..chunks {
            let offset = i * 8;
            let mut k = u64::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);

            k = k.wrapping_mul(c1);
            k = k.rotate_left(31);
            k = k.wrapping_mul(c2);

            h ^= k;
            h = h.rotate_left(27);
            h = h.wrapping_mul(5).wrapping_add(0x52dce729);

            processed += 8;
        }

        let remaining = &data[processed..];
        let mut k: u64 = 0;

        for (i, &byte) in remaining.iter().enumerate() {
            k |= (byte as u64) << (i * 8);
        }

        if !remaining.is_empty() {
            k = k.wrapping_mul(c1);
            k = k.rotate_left(31);
            k = k.wrapping_mul(c2);
            h ^= k;
        }

        h ^= data.len() as u64;

        h ^= h >> 33;
        h = h.wrapping_mul(0xff51afd7ed558ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
        h ^= h >> 33;

        h
    }

    pub fn clear(&mut self) {
        self.bitmap.fill(false);
        self.item_count = 0;
    }

    pub fn len(&self) -> usize {
        self.item_count
    }

    pub fn is_empty(&self) -> bool {
        self.item_count == 0
    }

    pub fn bit_count(&self) -> usize {
        self.bit_count
    }

    pub fn hash_count(&self) -> usize {
        self.hash_count
    }

    pub fn memory_usage(&self) -> usize {
        self.bit_count.div_ceil(8)
    }

    pub fn estimated_false_positive_rate(&self) -> f64 {
        if self.bit_count == 0 || self.item_count == 0 {
            return 0.0;
        }

        let k = self.hash_count as f64;
        let n = self.item_count as f64;
        let m = self.bit_count as f64;

        (1.0 - (-k * n / m).exp()).powf(k)
    }

    pub fn merge(&mut self, other: &BloomFilter) {
        if self.bit_count != other.bit_count {
            return;
        }

        for i in 0..self.bit_count {
            if other.bitmap[i] {
                self.bitmap.set(i, true);
            }
        }

        self.item_count = std::cmp::max(self.item_count, other.item_count);
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(16 + self.memory_usage());

        result.extend_from_slice(&(self.bit_count as u64).to_le_bytes());
        result.extend_from_slice(&(self.hash_count as u64).to_le_bytes());
        result.extend_from_slice(&(self.item_count as u64).to_le_bytes());

        for byte in self.bitmap.as_raw_slice() {
            result.push(*byte);
        }

        result
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 24 {
            return None;
        }

        let bit_count = u64::from_le_bytes(data[0..8].try_into().ok()?) as usize;
        let hash_count = u64::from_le_bytes(data[8..16].try_into().ok()?) as usize;
        let item_count = u64::from_le_bytes(data[16..24].try_into().ok()?) as usize;

        let raw_data = &data[24..];
        let expected_bytes = bit_count.div_ceil(8);

        if raw_data.len() < expected_bytes {
            return None;
        }

        let bitmap: BitVec<u8, Lsb0> = raw_data[..expected_bytes].iter().copied().collect();

        Some(Self {
            bitmap,
            hash_count,
            bit_count,
            item_count,
        })
    }
}

impl Default for BloomFilter {
    fn default() -> Self {
        Self::with_capacity(1024)
    }
}

#[derive(Debug, Clone)]
pub struct ScalableBloomFilter {
    filters: Vec<BloomFilter>,
    growth_factor: f64,
    tightening_ratio: f64,
    capacity: usize,
    false_positive_rate: f64,
}

impl ScalableBloomFilter {
    pub fn new(initial_capacity: usize, false_positive_rate: f64) -> Self {
        let filter = BloomFilter::new(initial_capacity, false_positive_rate);

        Self {
            filters: vec![filter],
            growth_factor: 2.0,
            tightening_ratio: 0.9,
            capacity: initial_capacity,
            false_positive_rate,
        }
    }

    pub fn insert(&mut self, key: &[u8]) {
        let current_filter = self.filters.last_mut().unwrap();

        if current_filter.len() >= self.capacity {
            let new_capacity = (self.capacity as f64 * self.growth_factor) as usize;
            let new_fpr = self.false_positive_rate * self.tightening_ratio;

            let new_filter = BloomFilter::new(new_capacity, new_fpr);
            self.filters.push(new_filter);
            self.capacity = new_capacity;
            self.false_positive_rate = new_fpr;
        }

        self.filters.last_mut().unwrap().insert(key);
    }

    pub fn might_contain(&self, key: &[u8]) -> bool {
        self.filters.iter().any(|f| f.might_contain(key))
    }

    pub fn len(&self) -> usize {
        self.filters.iter().map(|f| f.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.filters.iter().all(|f| f.is_empty())
    }

    pub fn memory_usage(&self) -> usize {
        self.filters.iter().map(|f| f.memory_usage()).sum()
    }

    pub fn filter_count(&self) -> usize {
        self.filters.len()
    }
}

impl Default for ScalableBloomFilter {
    fn default() -> Self {
        Self::new(1024, 0.01)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_filter_basic() {
        let mut filter = BloomFilter::with_capacity(100);

        filter.insert(b"hello");
        filter.insert(b"world");

        assert!(filter.might_contain(b"hello"));
        assert!(filter.might_contain(b"world"));
        assert!(!filter.might_contain(b"missing"));
    }

    #[test]
    fn test_bloom_filter_str() {
        let mut filter = BloomFilter::with_capacity(100);

        filter.insert_str("test1");
        filter.insert_str("test2");

        assert!(filter.might_contain_str("test1"));
        assert!(filter.might_contain_str("test2"));
    }

    #[test]
    fn test_bloom_filter_u64() {
        let mut filter = BloomFilter::with_capacity(100);

        filter.insert_u64(42);
        filter.insert_u64(100);

        assert!(filter.might_contain_u64(42));
        assert!(filter.might_contain_u64(100));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let expected_items = 1000;
        let fpr = 0.01;
        let mut filter = BloomFilter::new(expected_items, fpr);

        for i in 0..expected_items {
            filter.insert(&i.to_le_bytes());
        }

        let mut false_positives = 0;
        let test_count = 10000;

        for i in expected_items..(expected_items + test_count) {
            if filter.might_contain(&i.to_le_bytes()) {
                false_positives += 1;
            }
        }

        let actual_fpr = false_positives as f64 / test_count as f64;
        assert!(
            actual_fpr < fpr * 2.0,
            "Actual FPR {} exceeds threshold {}",
            actual_fpr,
            fpr * 2.0
        );
    }

    #[test]
    fn test_bloom_filter_clear() {
        let mut filter = BloomFilter::with_capacity(100);

        filter.insert(b"test");
        assert!(filter.might_contain(b"test"));

        filter.clear();
        assert!(!filter.might_contain(b"test"));
        assert!(filter.is_empty());
    }

    #[test]
    fn test_bloom_filter_merge() {
        let mut filter1 = BloomFilter::with_capacity(100);
        let mut filter2 = BloomFilter::with_capacity(100);

        filter1.insert(b"key1");
        filter2.insert(b"key2");

        filter1.merge(&filter2);

        assert!(filter1.might_contain(b"key1"));
        assert!(filter1.might_contain(b"key2"));
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let mut filter = BloomFilter::new(1000, 0.01);

        for i in 0u64..100 {
            filter.insert(&i.to_le_bytes());
        }

        let bytes = filter.to_bytes();
        let restored = BloomFilter::from_bytes(&bytes).unwrap();

        assert_eq!(filter.bit_count(), restored.bit_count());
        assert_eq!(filter.hash_count(), restored.hash_count());

        for i in 0u64..100 {
            assert!(restored.might_contain(&i.to_le_bytes()));
        }
    }

    #[test]
    fn test_bloom_filter_memory_usage() {
        let filter = BloomFilter::new(1000, 0.01);
        let expected = filter.bit_count().div_ceil(8);

        assert_eq!(filter.memory_usage(), expected);
    }

    #[test]
    fn test_scalable_bloom_filter() {
        let mut filter = ScalableBloomFilter::new(100, 0.01);

        for i in 0u64..500 {
            filter.insert(&i.to_le_bytes());
        }

        assert!(filter.might_contain(&0u64.to_le_bytes()));
        assert!(filter.might_contain(&499u64.to_le_bytes()));
        assert!(filter.filter_count() > 1);
    }

    #[test]
    fn test_scalable_bloom_filter_memory() {
        let mut filter = ScalableBloomFilter::new(100, 0.01);

        for i in 0u64..500 {
            filter.insert(&i.to_le_bytes());
        }

        let memory = filter.memory_usage();
        assert!(memory > 0);
        assert!(memory < 500 * 8);
    }
}

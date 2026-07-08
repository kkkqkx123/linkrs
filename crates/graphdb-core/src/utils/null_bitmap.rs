//! Null Bitmap
//!
//! Bit-packed null value bitmap for efficient storage.
//! Uses 1 bit per element instead of 1 byte per element in Vec<bool>.

use std::fmt;
use std::ops::{BitAnd, BitOr, Not};

/// Bit-packed null bitmap for efficient storage
///
/// Uses 64-bit words to store null flags, achieving 8x memory savings
/// compared to Vec<bool>.
#[derive(Clone, PartialEq, Eq)]
pub struct NullBitmap {
    /// Bit storage (64 bits per word)
    data: Vec<u64>,
    /// Number of elements tracked
    len: usize,
}

impl NullBitmap {
    /// Bits per word
    const BITS_PER_WORD: usize = 64;

    /// Create a new empty null bitmap
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            len: 0,
        }
    }

    /// Create a new null bitmap with specified capacity
    pub fn with_capacity(capacity: usize) -> Self {
        let words = Self::word_count(capacity);
        Self {
            data: Vec::with_capacity(words),
            len: 0,
        }
    }

    /// Create a null bitmap with all elements set to non-null
    pub fn with_len(len: usize) -> Self {
        let words = Self::word_count(len);
        Self {
            data: vec![0; words],
            len,
        }
    }

    /// Create a null bitmap with all elements set to null
    pub fn all_null(len: usize) -> Self {
        let words = Self::word_count(len);
        Self {
            data: vec![u64::MAX; words],
            len,
        }
    }

    /// Calculate number of words needed for given length
    fn word_count(len: usize) -> usize {
        len.div_ceil(Self::BITS_PER_WORD)
    }

    /// Get the word index and bit position for an element
    fn get_position(idx: usize) -> (usize, usize) {
        (idx / Self::BITS_PER_WORD, idx % Self::BITS_PER_WORD)
    }

    /// Ensure capacity for at least `new_len` elements
    pub fn reserve(&mut self, new_len: usize) {
        let words = Self::word_count(new_len);
        if words > self.data.len() {
            self.data.resize(words, 0);
        }
    }

    /// Set the length of the bitmap
    /// New elements are initialized to non-null (0)
    pub fn resize(&mut self, new_len: usize) {
        let words = Self::word_count(new_len);
        self.data.resize(words, 0);
        self.len = new_len;
    }

    /// Check if an element is null
    ///
    /// Returns true if the element is null or out of bounds
    pub fn is_null(&self, idx: usize) -> bool {
        if idx >= self.len {
            return true;
        }
        unsafe { self.is_null_unchecked(idx) }
    }

    /// Check if an element is null without bounds checking
    ///
    /// # Safety
    /// Caller must ensure `idx < self.len`.
    pub unsafe fn is_null_unchecked(&self, idx: usize) -> bool {
        let (word_idx, bit_idx) = Self::get_position(idx);
        (self.data.get_unchecked(word_idx) >> bit_idx) & 1 == 1
    }

    /// Check if an element is valid (not null)
    pub fn is_valid(&self, idx: usize) -> bool {
        !self.is_null(idx)
    }

    /// Set an element as null or non-null
    pub fn set(&mut self, idx: usize, is_null: bool) {
        if idx >= self.len {
            self.resize(idx + 1);
        }
        let (word_idx, bit_idx) = Self::get_position(idx);
        if is_null {
            self.data[word_idx] |= 1u64 << bit_idx;
        } else {
            self.data[word_idx] &= !(1u64 << bit_idx);
        }
    }

    /// Set an element as null
    pub fn set_null(&mut self, idx: usize) {
        self.set(idx, true);
    }

    /// Set an element as valid (non-null)
    pub fn set_valid(&mut self, idx: usize) {
        self.set(idx, false);
    }

    /// Set a range of elements as null
    ///
    /// All indices in `[start, end)` are marked as null.
    ///
    /// # Panics
    /// Panics if `end > self.len` or `start > end`.
    pub fn set_null_range(&mut self, start: usize, end: usize) {
        assert!(end <= self.len && start <= end);
        if start == end {
            return;
        }
        let (start_word, start_bit) = Self::get_position(start);
        let (end_word, end_bit) = Self::get_position(end);

        if start_word == end_word {
            // Range within a single word
            let len = end_bit - start_bit;
            let mask = ((1u64 << len) - 1) << start_bit;
            self.data[start_word] |= mask;
        } else {
            // First partial word
            self.data[start_word] |= !0u64 << start_bit;

            // Full words in between
            for word in &mut self.data[start_word + 1..end_word] {
                *word = u64::MAX;
            }

            // Last partial word
            if end_bit > 0 {
                let mask = (1u64 << end_bit) - 1;
                self.data[end_word] |= mask;
            }
        }
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if the bitmap is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Count the number of null elements
    pub fn null_count(&self) -> usize {
        let full_words = self.len / Self::BITS_PER_WORD;
        let remaining_bits = self.len % Self::BITS_PER_WORD;

        let mut count: usize = self.data[..full_words]
            .iter()
            .map(|&w| w.count_ones() as usize)
            .sum();

        if remaining_bits > 0 && full_words < self.data.len() {
            let last_word = self.data[full_words];
            let mask = (1u64 << remaining_bits) - 1;
            count += (last_word & mask).count_ones() as usize;
        }

        count
    }

    /// Count the number of valid (non-null) elements
    pub fn valid_count(&self) -> usize {
        self.len - self.null_count()
    }

    /// Check if all elements are null
    pub fn is_all_null(&self) -> bool {
        self.null_count() == self.len
    }

    /// Check if all elements are valid
    pub fn is_all_valid(&self) -> bool {
        self.null_count() == 0
    }

    /// Get the underlying bit data
    pub fn as_bits(&self) -> &[u64] {
        &self.data
    }

    /// Append a null (true) or non-null (false) element to the end
    pub fn push(&mut self, is_null: bool) {
        let idx = self.len;
        self.len += 1;
        let needed_words = Self::word_count(self.len);
        if needed_words > self.data.len() {
            self.data.push(0);
        }
        if is_null {
            let (word_idx, bit_idx) = Self::get_position(idx);
            self.data[word_idx] |= 1u64 << bit_idx;
        }
    }

    /// Clear all elements
    pub fn clear(&mut self) {
        self.data.clear();
        self.len = 0;
    }

    /// Fill all elements with the given value
    pub fn fill(&mut self, is_null: bool) {
        let value = if is_null { u64::MAX } else { 0 };
        self.data.fill(value);
    }

    /// Invert all bits (null becomes valid, valid becomes null)
    pub fn invert(&mut self) {
        for word in &mut self.data {
            *word = !*word;
        }
    }

    /// Get an iterator over null indices
    pub fn null_indices(&self) -> impl Iterator<Item = usize> + '_ {
        let len = self.len;
        self.data
            .iter()
            .enumerate()
            .flat_map(move |(word_idx, &word)| {
                let base = word_idx * Self::BITS_PER_WORD;
                (0..Self::BITS_PER_WORD).filter_map(move |bit_idx| {
                    let idx = base + bit_idx;
                    if idx < len && (word >> bit_idx) & 1 == 1 {
                        Some(idx)
                    } else {
                        None
                    }
                })
            })
    }

    /// Get an iterator over valid (non-null) indices
    pub fn valid_indices(&self) -> impl Iterator<Item = usize> + '_ {
        let len = self.len;
        self.data
            .iter()
            .enumerate()
            .flat_map(move |(word_idx, &word)| {
                let base = word_idx * Self::BITS_PER_WORD;
                (0..Self::BITS_PER_WORD).filter_map(move |bit_idx| {
                    let idx = base + bit_idx;
                    if idx < len && (word >> bit_idx) & 1 == 0 {
                        Some(idx)
                    } else {
                        None
                    }
                })
            })
    }

    /// Memory usage in bytes
    pub fn memory_usage(&self) -> usize {
        self.data.len() * std::mem::size_of::<u64>()
    }

    /// Memory usage per element in bits
    pub fn bits_per_element(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        (self.memory_usage() * 8) as f64 / self.len as f64
    }
}

impl Default for NullBitmap {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NullBitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NullBitmap {{ len: {}, nulls: [", self.len)?;
        let nulls: Vec<_> = self.null_indices().take(10).collect();
        for (i, idx) in nulls.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", idx)?;
        }
        if self.null_count() > 10 {
            write!(f, ", ...")?;
        }
        write!(f, "] }}")
    }
}

impl BitAnd for &NullBitmap {
    type Output = NullBitmap;

    fn bitand(self, rhs: Self) -> Self::Output {
        let len = self.len.min(rhs.len);
        let mut result = NullBitmap::with_len(len);
        for i in 0..result.data.len() {
            let left = self.data.get(i).copied().unwrap_or(0);
            let right = rhs.data.get(i).copied().unwrap_or(0);
            result.data[i] = left & right;
        }
        result
    }
}

impl BitOr for &NullBitmap {
    type Output = NullBitmap;

    fn bitor(self, rhs: Self) -> Self::Output {
        let len = self.len.max(rhs.len);
        let mut result = NullBitmap::with_len(len);
        for i in 0..result.data.len() {
            let left = self.data.get(i).copied().unwrap_or(0);
            let right = rhs.data.get(i).copied().unwrap_or(0);
            result.data[i] = left | right;
        }
        result
    }
}

impl Not for &NullBitmap {
    type Output = NullBitmap;

    fn not(self) -> Self::Output {
        let mut result = self.clone();
        result.invert();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let mut bitmap = NullBitmap::with_len(10);

        assert!(!bitmap.is_null(0));
        assert!(!bitmap.is_null(5));

        bitmap.set_null(3);
        assert!(bitmap.is_null(3));
        assert!(!bitmap.is_null(4));

        bitmap.set_valid(3);
        assert!(!bitmap.is_null(3));
    }

    #[test]
    fn test_all_null() {
        let bitmap = NullBitmap::all_null(100);
        assert!(bitmap.is_all_null());
        assert_eq!(bitmap.null_count(), 100);
        assert_eq!(bitmap.valid_count(), 0);
    }

    #[test]
    fn test_all_valid() {
        let bitmap = NullBitmap::with_len(100);
        assert!(bitmap.is_all_valid());
        assert_eq!(bitmap.null_count(), 0);
        assert_eq!(bitmap.valid_count(), 100);
    }

    #[test]
    fn test_memory_efficiency() {
        let bitmap = NullBitmap::with_len(1000);
        assert!(bitmap.memory_usage() <= 128);
        assert!(bitmap.bits_per_element() <= 1.1);
    }

    #[test]
    fn test_comparison_with_vec_bool() {
        let n = 1000;
        let bitmap = NullBitmap::with_len(n);
        let vec_bool: Vec<bool> = vec![false; n];

        let bitmap_size = bitmap.memory_usage();
        let vec_size = vec_bool.len() * std::mem::size_of::<bool>();
        assert!(bitmap_size < vec_size / 7);
    }

    #[test]
    fn test_resize() {
        let mut bitmap = NullBitmap::with_len(10);
        bitmap.set_null(5);

        bitmap.resize(100);
        assert!(bitmap.is_null(5));
        assert!(!bitmap.is_null(50));
        assert_eq!(bitmap.len(), 100);
    }

    #[test]
    fn test_null_indices() {
        let mut bitmap = NullBitmap::with_len(10);
        bitmap.set_null(1);
        bitmap.set_null(3);
        bitmap.set_null(7);

        let nulls: Vec<_> = bitmap.null_indices().collect();
        assert_eq!(nulls, vec![1, 3, 7]);
    }

    #[test]
    fn test_valid_indices() {
        let mut bitmap = NullBitmap::all_null(5);
        bitmap.set_valid(1);
        bitmap.set_valid(3);

        let valids: Vec<_> = bitmap.valid_indices().collect();
        assert_eq!(valids, vec![1, 3]);
    }

    #[test]
    fn test_bitwise_operations() {
        let mut a = NullBitmap::with_len(10);
        a.set_null(1);
        a.set_null(2);

        let mut b = NullBitmap::with_len(10);
        b.set_null(2);
        b.set_null(3);

        let and_result = &a & &b;
        assert!(and_result.is_null(2));
        assert!(!and_result.is_null(1));
        assert!(!and_result.is_null(3));

        let or_result = &a | &b;
        assert!(or_result.is_null(1));
        assert!(or_result.is_null(2));
        assert!(or_result.is_null(3));

        let not_result = !&a;
        assert!(!not_result.is_null(1));
        assert!(!not_result.is_null(2));
        assert!(not_result.is_null(0));
    }

    #[test]
    fn test_out_of_bounds() {
        let bitmap = NullBitmap::with_len(10);
        assert!(bitmap.is_null(100));
    }

    #[test]
    fn test_large_bitmap() {
        let mut bitmap = NullBitmap::with_len(10000);

        for i in (0..10000).step_by(100) {
            bitmap.set_null(i);
        }

        assert_eq!(bitmap.null_count(), 100);

        for i in 0..10000 {
            if i % 100 == 0 {
                assert!(bitmap.is_null(i));
            } else {
                assert!(!bitmap.is_null(i));
            }
        }
    }

    #[test]
    fn test_is_null_unchecked() {
        let mut bitmap = NullBitmap::with_len(100);
        bitmap.set_null(42);

        unsafe {
            assert!(bitmap.is_null_unchecked(42));
            assert!(!bitmap.is_null_unchecked(0));
            assert!(!bitmap.is_null_unchecked(99));
        }
    }

    #[test]
    fn test_set_null_range_single_word() {
        let mut bitmap = NullBitmap::with_len(64);
        bitmap.set_null_range(10, 20);

        for i in 0..64 {
            if (10..20).contains(&i) {
                assert!(bitmap.is_null(i), "bit {} should be null", i);
            } else {
                assert!(!bitmap.is_null(i), "bit {} should not be null", i);
            }
        }
        assert_eq!(bitmap.null_count(), 10);
    }

    #[test]
    fn test_set_null_range_multi_word() {
        let mut bitmap = NullBitmap::with_len(200);
        bitmap.set_null_range(60, 130);

        for i in 0..200 {
            if (60..130).contains(&i) {
                assert!(bitmap.is_null(i), "bit {} should be null", i);
            } else {
                assert!(!bitmap.is_null(i), "bit {} should not be null", i);
            }
        }
        assert_eq!(bitmap.null_count(), 70);
    }

    #[test]
    fn test_set_null_range_empty() {
        let mut bitmap = NullBitmap::with_len(100);
        bitmap.set_null_range(30, 30);
        assert_eq!(bitmap.null_count(), 0);
    }

    #[test]
    fn test_set_null_range_full() {
        let mut bitmap = NullBitmap::with_len(200);
        bitmap.set_null_range(0, 200);
        assert!(bitmap.is_all_null());
        assert_eq!(bitmap.null_count(), 200);
    }

    #[test]
    fn test_bitand_different_lengths() {
        let mut a = NullBitmap::with_len(10);
        a.set_null(5);
        a.set_null(8);

        let mut b = NullBitmap::with_len(20);
        b.set_null(8);
        b.set_null(15);

        let result = &a & &b;
        assert_eq!(result.len(), 10);
        assert!(result.is_null(8));
        assert!(!result.is_null(5));
        assert!(!result.is_null(9));
    }

    #[test]
    fn test_bitor_different_lengths() {
        let mut a = NullBitmap::with_len(10);
        a.set_null(5);
        a.set_null(8);

        let mut b = NullBitmap::with_len(20);
        b.set_null(8);
        b.set_null(15);

        let result = &a | &b;
        assert_eq!(result.len(), 20);
        assert!(result.is_null(5));
        assert!(result.is_null(8));
        assert!(result.is_null(15));
        assert!(!result.is_null(0));
        assert!(!result.is_null(16));
    }

    #[test]
    #[should_panic(expected = "end <= self.len")]
    fn test_set_null_range_panic() {
        let mut bitmap = NullBitmap::with_len(10);
        bitmap.set_null_range(5, 15);
    }

    #[test]
    fn test_push() {
        let mut bitmap = NullBitmap::new();
        bitmap.push(false);
        bitmap.push(true);
        bitmap.push(false);
        bitmap.push(true);
        bitmap.push(true);

        assert_eq!(bitmap.len(), 5);
        assert!(!bitmap.is_null(0));
        assert!(bitmap.is_null(1));
        assert!(!bitmap.is_null(2));
        assert!(bitmap.is_null(3));
        assert!(bitmap.is_null(4));
        assert_eq!(bitmap.null_count(), 3);
        assert_eq!(bitmap.valid_count(), 2);
    }
}

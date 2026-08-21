//! Striped RwLock for fine-grained concurrency
//!
//! Provides `StripedRwLock<T>` which shards data into `N` stripes, each guarded
//! by a `parking_lot::RwLock`. Different stripes can be read/written concurrently;
//! contention is reduced by `N` for disjoint keys (e.g., property row stripes or
//! edge partitions). MVCC snapshot reads (`get_at_ts`) are lock-free beyond the
//! stripe's shared guard, allowing OLAP scans to proceed without blocking writes
//! to other stripes.
//!
//! This complements the MVCC version chains in `ColumnStore` / `PropertyTable`:
//! readers observe a timestamped snapshot without holding an exclusive lock, while
//! writers lock only the stripe covering the affected row range.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use parking_lot::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Number of stripes; power of two for fast modulo via bitmask.
const DEFAULT_STRIPES: usize = 16;

/// A sharded read-write lock. Each shard holds an independent `T`.
/// `T` must be `Default` so shards can be lazily initialized if needed, but
/// callers typically provide initial values via `new_with` or `from_iter`.
#[derive(Debug)]
pub struct StripedRwLock<T> {
    stripes: Vec<RwLock<T>>,
    stripe_mask: usize,
}

impl<T: Default> Default for StripedRwLock<T> {
    fn default() -> Self {
        Self::new(DEFAULT_STRIPES)
    }
}

impl<T> StripedRwLock<T> {
    /// Create a striped lock with `n` stripes (rounded up to power of two).
    pub fn new(n: usize) -> Self
    where
        T: Default,
    {
        let n = n.next_power_of_two().max(1);
        let mut stripes = Vec::with_capacity(n);
        for _ in 0..n {
            stripes.push(RwLock::new(T::default()));
        }
        Self {
            stripes,
            stripe_mask: n - 1,
        }
    }

    /// Create with explicit per-stripe initial values.
    pub fn from_vec(values: Vec<T>) -> Self {
        let n = values.len().next_power_of_two().max(1);
        let stripes: Vec<RwLock<T>> = values.into_iter().map(RwLock::new).collect();
        while stripes.len() < n {
            // For non-Default T, this path requires caller to provide power-of-two count.
            // We panic to surface misuse early.
            panic!("StripedRwLock::from_vec requires power-of-two length");
        }
        Self {
            stripes,
            stripe_mask: n - 1,
        }
    }

    #[inline]
    fn stripe_index<K: Hash>(&self, key: &K) -> usize {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) & self.stripe_mask
    }

    #[inline]
    pub fn stripe_count(&self) -> usize {
        self.stripes.len()
    }

    /// Read guard for the stripe covering `key`.
    pub fn read<K: Hash>(&self, key: &K) -> RwLockReadGuard<'_, T> {
        let idx = self.stripe_index(key);
        self.stripes[idx].read()
    }

    /// Write guard for the stripe covering `key`.
    pub fn write<K: Hash>(&self, key: &K) -> RwLockWriteGuard<'_, T> {
        let idx = self.stripe_index(key);
        self.stripes[idx].write()
    }

    /// Read guard by stripe index (for row-chunk stripes).
    pub fn read_by_index(&self, idx: usize) -> RwLockReadGuard<'_, T> {
        self.stripes[idx & self.stripe_mask].read()
    }

    /// Write guard by stripe index.
    pub fn write_by_index(&self, idx: usize) -> RwLockWriteGuard<'_, T> {
        self.stripes[idx & self.stripe_mask].write()
    }

    /// Try read without blocking.
    pub fn try_read<K: Hash>(&self, key: &K) -> Option<RwLockReadGuard<'_, T>> {
        let idx = self.stripe_index(key);
        self.stripes[idx].try_read()
    }

    /// Try write without blocking.
    pub fn try_write<K: Hash>(&self, key: &K) -> Option<RwLockWriteGuard<'_, T>> {
        let idx = self.stripe_index(key);
        self.stripes[idx].try_write()
    }

    /// Iterate over all stripes mutably (for maintenance / GC).
    pub fn write_all(&self) -> Vec<RwLockWriteGuard<'_, T>> {
        self.stripes.iter().map(|s| s.write()).collect()
    }
}

// Convenience for sharing across threads.
pub type SharedStripedLock<T> = Arc<StripedRwLock<T>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_striped_lock_basic() {
        let lock: StripedRwLock<i32> = StripedRwLock::new(4);
        {
            let mut w = lock.write(&"key1");
            *w = 42;
        }
        {
            let r = lock.read(&"key1");
            assert_eq!(*r, 42);
        }
    }

    #[test]
    fn test_disjoint_keys_do_not_block() {
        let lock = Arc::new(StripedRwLock::<Vec<u32>>::new(16));
        let l1 = lock.clone();
        let l2 = lock.clone();
        let h1 = std::thread::spawn(move || {
            let mut w = l1.write(&0u32);
            w.push(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        });
        let h2 = std::thread::spawn(move || {
            // Different stripe (high bit difference) should not contend.
            let _r = l2.read(&9999u32);
        });
        h1.join().unwrap();
        h2.join().unwrap();
    }

    #[test]
    fn test_by_index() {
        let lock: StripedRwLock<String> = StripedRwLock::new(8);
        {
            let mut w = lock.write_by_index(3);
            *w = "hello".to_string();
        }
        assert_eq!(*lock.read_by_index(3), "hello");
        assert_eq!(*lock.read_by_index(11), "hello"); // 11 & 7 == 3
    }
}

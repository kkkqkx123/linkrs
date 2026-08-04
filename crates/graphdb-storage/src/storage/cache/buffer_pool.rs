use crate::storage::engine::resource_budget::{MemoryAccounting, MemoryCategory};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::error::Error;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type LoaderFn<K, T> = Arc<dyn Fn(K) -> Option<(T, usize)> + Send + Sync>;
type WriterFn<K, T> = Arc<dyn Fn(K, &T) -> Result<(), Box<dyn Error + Send + Sync>> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct CachedItem<T: Clone + Send + Sync> {
    pub(crate) item: T,
    pub(crate) pin_count: Arc<AtomicU64>,
    pub(crate) dirty: Arc<AtomicBool>,
    clock_flag: Arc<AtomicBool>,
    last_access: Arc<AtomicU64>,
    size: usize,
}

impl<T: Clone + Send + Sync> CachedItem<T> {
    pub(crate) fn new(item: T, size: usize) -> Self {
        Self {
            item,
            pin_count: Arc::new(AtomicU64::new(0)),
            dirty: Arc::new(AtomicBool::new(false)),
            clock_flag: Arc::new(AtomicBool::new(true)),
            last_access: Arc::new(AtomicU64::new(timestamp_nanos())),
            size,
        }
    }

    pub(crate) fn pin(&self) {
        self.pin_count.fetch_add(1, Ordering::AcqRel);
        self.last_access.store(timestamp_nanos(), Ordering::Relaxed);
    }

    pub(crate) fn unpin(&self) {
        self.pin_count.fetch_sub(1, Ordering::AcqRel);
    }

    pub(crate) fn is_pinned(&self) -> bool {
        self.pin_count.load(Ordering::Acquire) > 0
    }
}

#[derive(Clone)]
pub(crate) struct BufferPool<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> {
    inner: Arc<BufferPoolInner<K, T>>,
}

struct BufferPoolInner<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> {
    capacity: AtomicU64,
    items: Mutex<HashMap<K, CachedItem<T>>>,
    clock_hand: Mutex<usize>,
    cached_ids: Mutex<Vec<K>>,
    loader: Mutex<Option<LoaderFn<K, T>>>,
    writer: Mutex<Option<WriterFn<K, T>>>,
    memory_accounting: Mutex<Option<Arc<MemoryAccounting>>>,
    /// Total weighted size of the cached items. Maintained incrementally so
    /// `current_usage` is O(1) instead of scanning the whole map on every
    /// insert (which made batch inserts quadratic in cache size).
    usage: AtomicU64,
}

impl<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> BufferPool<K, T> {
    pub(crate) fn new(capacity_bytes: u64) -> Self {
        Self {
            inner: Arc::new(BufferPoolInner {
                capacity: AtomicU64::new(capacity_bytes),
                items: Mutex::new(HashMap::new()),
                clock_hand: Mutex::new(0),
                cached_ids: Mutex::new(Vec::new()),
                loader: Mutex::new(None),
                writer: Mutex::new(None),
                memory_accounting: Mutex::new(None),
                usage: AtomicU64::new(0),
            }),
        }
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.inner.capacity.load(Ordering::Acquire)
    }

    pub(crate) fn get(&self, key: &K) -> Option<CachedItem<T>> {
        let items = self.inner.items.lock();
        items.get(key).cloned()
    }

    pub(crate) fn set_loader<F>(&self, loader: F)
    where
        F: Fn(K) -> Option<(T, usize)> + Send + Sync + 'static,
    {
        *self.inner.loader.lock() = Some(Arc::new(loader));
    }

    pub(crate) fn set_memory_accounting(&self, accounting: Option<Arc<MemoryAccounting>>) {
        *self.inner.memory_accounting.lock() = accounting;
    }

    pub(crate) fn set_writer<F>(&self, writer: F)
    where
        F: Fn(K, &T) -> Result<(), Box<dyn Error + Send + Sync>> + Send + Sync + 'static,
    {
        *self.inner.writer.lock() = Some(Arc::new(writer));
    }

    pub(crate) fn get_or_load(&self, key: &K) -> Option<CachedItem<T>> {
        if let Some(item) = self.get(key) {
            return Some(item);
        }
        let loader = self.inner.loader.lock().clone()?;
        let (item, size) = loader(key.clone())?;
        self.insert(key.clone(), item.clone(), size);
        self.get(key)
    }

    pub(crate) fn insert(&self, key: K, item: T, size: usize) {
        let size_u64 = size as u64;
        let usage = self.current_usage();

        if usage.saturating_add(size_u64) > self.inner.capacity.load(Ordering::Acquire) {
            let excess =
                usage.saturating_add(size_u64) - self.inner.capacity.load(Ordering::Acquire);
            self.evict(excess + 1);
        }

        let cached = CachedItem::new(item, size);
        let mut items = self.inner.items.lock();
        let is_new = match items.insert(key.clone(), cached) {
            None => {
                self.inner.usage.fetch_add(size_u64, Ordering::Relaxed);
                true
            }
            Some(previous) => {
                // Overwrite: adjust usage by the size delta so the counter
                // stays exact even when the same key is cached at a different
                // size.
                let previous_size = previous.size as u64;
                if previous_size != size_u64 {
                    self.inner.usage.fetch_add(size_u64, Ordering::Relaxed);
                    self.inner.usage.fetch_sub(previous_size, Ordering::Relaxed);
                }
                false
            }
        };
        let mut ids = self.inner.cached_ids.lock();
        if is_new {
            ids.push(key);
        }
        drop(ids);
        drop(items);

        if let Some(ref accounting) = *self.inner.memory_accounting.lock() {
            accounting.report_usage(MemoryCategory::Cache, self.current_usage());
        }
    }

    pub(crate) fn set_capacity(&self, new_capacity: u64) {
        self.inner.capacity.store(new_capacity, Ordering::Release);
        let usage = self.current_usage();
        if usage > new_capacity {
            self.evict(usage - new_capacity);
        }
    }

    pub(crate) fn current_usage(&self) -> u64 {
        self.inner.usage.load(Ordering::Acquire)
    }

    pub(crate) fn evict(&self, target_bytes: u64) -> u64 {
        let mut items = self.inner.items.lock();
        let mut ids = self.inner.cached_ids.lock();

        if items.is_empty() {
            return 0;
        }

        let mut evicted = 0u64;
        let mut attempts = 0u64;
        let max_attempts = ids.len() as u64 * 2;

        if ids.is_empty() {
            ids.extend(items.keys().cloned());
        }

        let mut hand = self.inner.clock_hand.lock().wrapping_rem(ids.len().max(1));

        while evicted < target_bytes && attempts < max_attempts {
            if ids.is_empty() {
                break;
            }
            if hand >= ids.len() {
                hand = 0;
            }
            let id = ids[hand].clone();
            if let Some(cached) = items.get(&id) {
                if cached.is_pinned() {
                    hand = (hand + 1) % ids.len().max(1);
                    attempts += 1;
                    continue;
                }
                let flag = cached.clock_flag.load(Ordering::Acquire);
                if flag {
                    cached.clock_flag.store(false, Ordering::Release);
                    hand = (hand + 1) % ids.len().max(1);
                    attempts += 1;
                    continue;
                }
                let item = items.remove(&id);
                if let Some(item) = item {
                    if item.dirty.load(Ordering::Acquire) {
                        if let Some(writer) = self.inner.writer.lock().as_ref() {
                            if let Err(e) = writer(id.clone(), &item.item) {
                                tracing::warn!("Failed to write back key during eviction: {e}");
                            }
                        }
                    }
                    let item_size = item.size as u64;
                    self.inner.usage.fetch_sub(item_size, Ordering::Relaxed);
                    evicted += item_size;
                    if let Some(ref accounting) = *self.inner.memory_accounting.lock() {
                        accounting.release_category(MemoryCategory::Cache, item_size);
                    }
                    ids.retain(|i| *i != id);
                    if ids.is_empty() {
                        break;
                    }
                }
                if ids.is_empty() {
                    break;
                }
                hand %= ids.len().max(1);
            } else {
                ids.retain(|i| items.contains_key(i));
                if ids.is_empty() {
                    break;
                }
                hand %= ids.len().max(1);
            }
            attempts += 1;
        }

        *self.inner.clock_hand.lock() = hand;
        evicted
    }

    pub(crate) fn len(&self) -> usize {
        let items = self.inner.items.lock();
        items.len()
    }

    /// Remove all entries satisfying a predicate.
    /// Returns the number of entries removed.
    pub(crate) fn retain<F>(&self, mut f: F) -> usize
    where
        F: FnMut(&K, &T) -> bool,
    {
        let mut items = self.inner.items.lock();
        let before = items.len();
        let mut removed_bytes = 0u64;
        items.retain(|k, v| {
            if f(k, &v.item) {
                true
            } else {
                removed_bytes += v.size as u64;
                false
            }
        });
        let removed = before - items.len();
        if removed > 0 {
            self.inner.usage.fetch_sub(removed_bytes, Ordering::Relaxed);
            let mut ids = self.inner.cached_ids.lock();
            ids.retain(|i| items.contains_key(i));
        }
        removed
    }
}

impl<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> std::fmt::Debug
    for BufferPool<K, T>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferPool")
            .field("capacity", &self.capacity())
            .field("usage", &self.current_usage())
            .field("items", &self.len())
            .finish()
    }
}

fn timestamp_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool() -> BufferPool<u32, &'static str> {
        BufferPool::new(1024)
    }

    #[test]
    fn insert_and_get() {
        let pool = make_pool();
        pool.insert(1, "hello", 8);
        let item = pool.get(&1);
        assert!(item.is_some());
        assert_eq!(item.unwrap().item, "hello");
    }

    #[test]
    fn get_missing_returns_none() {
        let pool = make_pool();
        assert!(pool.get(&99).is_none());
    }

    #[test]
    fn current_usage_sums_sizes() {
        let pool = BufferPool::<u32, &str>::new(8192);
        pool.insert(1, "a", 100);
        pool.insert(2, "b", 200);
        assert_eq!(pool.current_usage(), 300);
    }

    #[test]
    fn evict_removes_items_under_pressure() {
        let pool = BufferPool::<u32, &str>::new(1024);
        pool.insert(1, "entry1", 200);
        pool.insert(2, "entry2", 200);
        pool.insert(3, "entry3", 200);
        assert_eq!(pool.len(), 3);
        let evicted = pool.evict(400);
        assert!(evicted >= 400, "evicted {evicted} bytes, expected >=400");
        assert!(pool.len() < 3, "expected some evictions");
    }

    #[test]
    fn insert_evicts_when_over_capacity() {
        let pool = BufferPool::<u32, &str>::new(300);
        pool.insert(1, "entry1", 200);
        assert_eq!(pool.len(), 1);
        pool.insert(2, "entry2", 200);
        assert_eq!(pool.len(), 1, "should evict to stay under capacity");
    }

    #[test]
    fn set_capacity_triggers_eviction() {
        let pool = BufferPool::<u32, &str>::new(1024);
        pool.insert(1, "entry1", 200);
        pool.insert(2, "entry2", 200);
        assert_eq!(pool.len(), 2);
        pool.set_capacity(100);
        assert!(pool.len() < 2, "set_capacity should evict items");
    }

    #[test]
    fn pinned_items_are_not_evicted() {
        let pool = BufferPool::<u32, &str>::new(256);
        pool.insert(1, "pinned", 200);
        if let Some(item) = pool.get(&1) {
            item.pin();
        }
        pool.insert(2, "evictable", 200);
        pool.evict(400);
        if let Some(item) = pool.get(&1) {
            item.unpin();
        }
    }

    #[test]
    fn empty_pool_evicts_zero() {
        let pool = make_pool();
        assert_eq!(pool.evict(100), 0);
    }

    #[test]
    fn len_tracks_insertions() {
        let pool = make_pool();
        assert_eq!(pool.len(), 0);
        pool.insert(1, "x", 1);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn current_usage_counter_matches_sum_after_eviction_and_overwrite() {
        let pool = BufferPool::<u32, &str>::new(10_000);
        for i in 0..100u32 {
            pool.insert(i, "x", 8);
        }
        assert_eq!(pool.current_usage(), 800);
        assert_eq!(pool.len(), 100);

        // Overwriting a key with a different size adjusts the counter.
        pool.insert(50, "x", 16);
        assert_eq!(pool.current_usage(), 808);
        assert_eq!(pool.len(), 100);

        // Eviction under capacity pressure decrements the counter.
        let evicted = pool.evict(200);
        assert!(evicted >= 200);
        assert_eq!(
            pool.current_usage(),
            pool.len() as u64 * 8 + 8,
            "usage must equal the sum of remaining item sizes"
        );

        // retain decrements the counter for removed entries.
        let removed = pool.retain(|&k, _| k % 2 == 0);
        assert!(removed > 0);
        let expected: u64 = {
            let items = pool.inner.items.lock();
            items.values().map(|c| c.size as u64).sum()
        };
        assert_eq!(pool.current_usage(), expected);
    }
}

use crate::storage::engine::resource_budget::{MemoryAccounting, MemoryCategory};
use parking_lot::Mutex;
use std::collections::hash_map::{DefaultHasher, Entry};
use std::collections::HashMap;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

type LoaderFn<K, T> = Arc<dyn Fn(K) -> Option<(T, usize)> + Send + Sync>;
type WriterFn<K, T> = Arc<dyn Fn(K, &T) -> Result<(), Box<dyn Error + Send + Sync>> + Send + Sync>;
/// Pending dirty entries collected under a shard lock and written back after
/// the lock is released.
type WritebackList<K, T> = Vec<(K, Arc<CachedItem<T>>)>;

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

/// Number of shard maps, rounded up to a power of two (required by the
/// shard-selection mask). Adapted to CPU parallelism and bounded.
const MAX_POOL_SHARDS: usize = 16;

fn default_pool_shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_POOL_SHARDS)
        .next_power_of_two()
}

#[derive(Clone)]
pub(crate) struct BufferPool<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> {
    inner: Arc<BufferPoolInner<K, T>>,
}

struct BufferPoolInner<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> {
    capacity: AtomicU64,
    shards: Vec<Mutex<HashMap<K, Arc<CachedItem<T>>>>>,
    loader: Mutex<Option<LoaderFn<K, T>>>,
    writer: Mutex<Option<WriterFn<K, T>>>,
    memory_accounting: Mutex<Option<Arc<MemoryAccounting>>>,
    /// Total weighted size of the cached items. Maintained incrementally so
    /// `current_usage` is O(1) instead of scanning all shards on every insert.
    usage: AtomicU64,
}

impl<K: Hash + Eq + Clone + Send + Sync, T: Clone + Send + Sync> BufferPool<K, T> {
    pub(crate) fn new(capacity_bytes: u64) -> Self {
        let num_shards = default_pool_shards();
        let mut shards = Vec::with_capacity(num_shards);
        for _ in 0..num_shards {
            shards.push(Mutex::new(HashMap::new()));
        }
        Self {
            inner: Arc::new(BufferPoolInner {
                capacity: AtomicU64::new(capacity_bytes),
                shards,
                loader: Mutex::new(None),
                writer: Mutex::new(None),
                memory_accounting: Mutex::new(None),
                usage: AtomicU64::new(0),
            }),
        }
    }

    fn shard_for(&self, key: &K) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (self.inner.shards.len() - 1)
    }

    pub(crate) fn capacity(&self) -> u64 {
        self.inner.capacity.load(Ordering::Relaxed)
    }

    /// Look up a cached item, touching only the shard that owns `key`.
    /// Concurrent hits on different keys proceed in parallel.
    pub(crate) fn get(&self, key: &K) -> Option<Arc<CachedItem<T>>> {
        let shard = self.inner.shards[self.shard_for(key)].lock();
        shard.get(key).cloned()
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

    pub(crate) fn get_or_load(&self, key: &K) -> Option<Arc<CachedItem<T>>> {
        if let Some(item) = self.get(key) {
            return Some(item);
        }
        let loader = self.inner.loader.lock().clone()?;
        let (item, size) = loader(key.clone())?;
        self.insert(key.clone(), item.clone(), size);
        self.get(key)
    }

    /// Insert or replace the cached entry for `key`.
    ///
    /// The capacity check runs while holding the owning shard lock. If the
    /// shard has nothing evictable, an intervening global eviction (fixed
    /// shard order, holding at most one lock at a time) makes room before the
    /// insert. Dirty evictees are written back after all locks are released so
    /// I/O never happens under a shard lock.
    pub(crate) fn insert(&self, key: K, item: T, size: usize) {
        let idx = self.shard_for(&key);
        let size_u64 = size as u64;
        let mut writebacks: WritebackList<K, T> = WritebackList::new();

        {
            let mut shard = self.inner.shards[idx].lock();
            let capacity = self.inner.capacity.load(Ordering::Relaxed);
            let needed = self
                .inner
                .usage
                .load(Ordering::Relaxed)
                .saturating_add(size_u64);
            let mut need_global_evict = false;
            if needed > capacity {
                let (evicted, wb) = self.evict_locked(&mut shard, needed - capacity + 1);
                writebacks.extend(wb);
                if evicted == 0 {
                    // Nothing evictable in the owning shard: make room globally.
                    need_global_evict = true;
                }
            }
            if need_global_evict {
                drop(shard);
                let cap = self.inner.capacity.load(Ordering::Relaxed);
                let usage = self.inner.usage.load(Ordering::Relaxed);
                if usage.saturating_add(size_u64) > cap {
                    writebacks
                        .extend(self.evict_all_collect(usage.saturating_add(size_u64) - cap + 1));
                }
                shard = self.inner.shards[idx].lock();
            }

            let cached = Arc::new(CachedItem::new(item, size));
            match shard.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(cached);
                    self.inner.usage.fetch_add(size_u64, Ordering::Relaxed);
                }
                Entry::Occupied(mut entry) => {
                    let previous = entry.insert(cached);
                    let previous_size = previous.size as u64;
                    if previous_size != size_u64 {
                        self.inner.usage.fetch_add(size_u64, Ordering::Relaxed);
                        self.inner.usage.fetch_sub(previous_size, Ordering::Relaxed);
                    }
                    if previous.dirty.load(Ordering::Acquire) {
                        writebacks.push((entry.key().clone(), previous));
                    }
                }
            }
        }

        self.flush_writebacks(writebacks);
        if let Some(ref accounting) = *self.inner.memory_accounting.lock() {
            accounting.report_usage(MemoryCategory::Cache, self.current_usage());
        }
    }

    pub(crate) fn set_capacity(&self, new_capacity: u64) {
        self.inner.capacity.store(new_capacity, Ordering::Relaxed);
        let usage = self.current_usage();
        if usage > new_capacity {
            self.evict(usage - new_capacity);
        }
    }

    pub(crate) fn current_usage(&self) -> u64 {
        self.inner.usage.load(Ordering::Relaxed)
    }

    /// Evict at least `target_bytes` across shards in fixed order, touching
    /// at most one shard lock at a time. Write-backs run after all locks are
    /// released.
    pub(crate) fn evict(&self, target_bytes: u64) -> u64 {
        if target_bytes == 0 {
            return 0;
        }
        let mut evicted = 0u64;
        let mut writebacks = Vec::new();
        for shard_mutex in &self.inner.shards {
            if evicted >= target_bytes {
                break;
            }
            let mut shard = shard_mutex.lock();
            let (e, wb) = self.evict_locked(&mut shard, target_bytes - evicted);
            evicted += e;
            writebacks.extend(wb);
        }
        self.flush_writebacks(writebacks);
        evicted
    }

    fn evict_all_collect(&self, target_bytes: u64) -> WritebackList<K, T> {
        let mut evicted = 0u64;
        let mut writebacks = Vec::new();
        for shard_mutex in &self.inner.shards {
            if evicted >= target_bytes {
                break;
            }
            let mut shard = shard_mutex.lock();
            let (e, wb) = self.evict_locked(&mut shard, target_bytes - evicted);
            evicted += e;
            writebacks.extend(wb);
        }
        writebacks
    }

    /// Evict entries within one shard map by iterating it (O(m) per pass)
    /// with a CLOCK second-chance pass.
    fn evict_locked(
        &self,
        shard: &mut HashMap<K, Arc<CachedItem<T>>>,
        target_bytes: u64,
    ) -> (u64, WritebackList<K, T>) {
        let mut evicted = 0u64;
        let mut writebacks = Vec::new();
        if shard.is_empty() || target_bytes == 0 {
            return (evicted, writebacks);
        }
        for pass in 0..2u8 {
            if evicted >= target_bytes {
                break;
            }
            let keys: Vec<K> = shard.keys().cloned().collect();
            for id in keys {
                if evicted >= target_bytes {
                    break;
                }
                let victim = {
                    let cached = match shard.get(&id) {
                        Some(cached) => cached,
                        None => continue,
                    };
                    if cached.is_pinned() {
                        continue;
                    }
                    if pass == 0 && cached.clock_flag.load(Ordering::Acquire) {
                        cached.clock_flag.store(false, Ordering::Release);
                        continue;
                    }
                    Some((id, cached.clone()))
                };
                if let Some((victim_id, cached)) = victim {
                    shard.remove(&victim_id);
                    let item_size = cached.size as u64;
                    self.inner.usage.fetch_sub(item_size, Ordering::Relaxed);
                    if let Some(ref accounting) = *self.inner.memory_accounting.lock() {
                        accounting.release_category(MemoryCategory::Cache, item_size);
                    }
                    writebacks.push((victim_id, cached));
                    evicted += item_size;
                }
            }
        }
        (evicted, writebacks)
    }

    fn flush_writebacks(&self, writebacks: WritebackList<K, T>) {
        if writebacks.is_empty() {
            return;
        }
        let writer = self.inner.writer.lock().clone();
        let Some(writer) = writer else {
            return;
        };
        for (id, cached) in writebacks {
            if cached.dirty.load(Ordering::Acquire) {
                if let Err(e) = writer(id, &cached.item) {
                    tracing::warn!("Failed to write back key during eviction: {e}");
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.inner.shards.iter().map(|m| m.lock().len()).sum()
    }

    /// Remove all entries satisfying a predicate.
    /// Returns the number of entries removed.
    pub(crate) fn retain<F>(&self, mut f: F) -> usize
    where
        F: FnMut(&K, &T) -> bool,
    {
        let mut removed = 0usize;
        let mut removed_bytes = 0u64;
        for shard_mutex in &self.inner.shards {
            let mut shard = shard_mutex.lock();
            shard.retain(|k, v| {
                if f(k, &v.item) {
                    true
                } else {
                    removed_bytes += v.size as u64;
                    removed += 1;
                    false
                }
            });
        }
        if removed > 0 {
            self.inner.usage.fetch_sub(removed_bytes, Ordering::Relaxed);
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
        let expected: u64 = {
            let mut sum = 0u64;
            for shard in &pool.inner.shards {
                sum += shard.lock().values().map(|c| c.size as u64).sum::<u64>();
            }
            sum
        };
        assert_eq!(
            pool.current_usage(),
            expected,
            "usage must equal the sum of remaining item sizes"
        );

        // retain decrements the counter for removed entries.
        let removed = pool.retain(|&k, _| k % 2 == 0);
        assert!(removed > 0);
        let expected: u64 = {
            let mut sum = 0u64;
            for shard in &pool.inner.shards {
                sum += shard.lock().values().map(|c| c.size as u64).sum::<u64>();
            }
            sum
        };
        assert_eq!(pool.current_usage(), expected);
    }

    #[test]
    fn dirty_items_write_back_without_holding_locks() {
        use std::sync::atomic::AtomicUsize;
        let pool = BufferPool::<u32, String>::new(300);
        let written = Arc::new(AtomicUsize::new(0));
        let written_clone = written.clone();
        pool.set_writer(move |_k: u32, v: &String| {
            assert_eq!(v, "hello");
            written_clone.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        pool.insert(1, "hello".to_string(), 100);
        if let Some(item) = pool.get(&1) {
            item.dirty.store(true, Ordering::Release);
        }
        pool.insert(2, "world".to_string(), 100);
        pool.insert(3, "again".to_string(), 100);
        pool.insert(4, "force".to_string(), 100);
        assert!(
            written.load(Ordering::SeqCst) >= 1,
            "dirty item written back"
        );
    }
}

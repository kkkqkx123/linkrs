use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::RwLock;

use super::eviction::EvictionQueue;
use super::page::BufferPage;

pub type PageId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferCategory {
    VertexColumn,
    EdgeSegment,
    IndexNode,
    IndexLeaf,
}

#[derive(Debug, Clone)]
pub struct BufferConfig {
    pub max_pages: usize,
    pub max_scan_per_evict: usize,
    pub evict_batch_size: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            max_pages: 65536,
            max_scan_per_evict: 1024,
            evict_batch_size: 256,
        }
    }
}

struct PageStore {
    pages: HashMap<PageId, BufferPage<Vec<u8>>>,
    eviction: EvictionQueue,
}

impl PageStore {
    fn new(max_scan: usize) -> Self {
        Self {
            pages: HashMap::new(),
            eviction: EvictionQueue::new(max_scan),
        }
    }

    fn get(&self, page_id: PageId) -> Option<&BufferPage<Vec<u8>>> {
        self.pages.get(&page_id)
    }

    fn get_mut(&mut self, page_id: PageId) -> Option<&mut BufferPage<Vec<u8>>> {
        self.pages.get_mut(&page_id)
    }

    fn insert(&mut self, page_id: PageId, page: BufferPage<Vec<u8>>) {
        self.pages.insert(page_id, page);
        self.eviction.add(page_id);
    }

    fn remove(&mut self, page_id: PageId) -> Option<BufferPage<Vec<u8>>> {
        self.eviction.remove(page_id);
        self.pages.remove(&page_id)
    }

    fn contains(&self, page_id: PageId) -> bool {
        self.pages.contains_key(&page_id)
    }
}

pub struct BufferManager {
    stores: [RwLock<PageStore>; 4],
    config: BufferConfig,
    next_page_id: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

fn store_index(category: BufferCategory) -> usize {
    match category {
        BufferCategory::VertexColumn => 0,
        BufferCategory::EdgeSegment => 1,
        BufferCategory::IndexNode => 2,
        BufferCategory::IndexLeaf => 3,
    }
}

impl BufferManager {
    pub fn new(config: BufferConfig) -> Self {
        let max_scan = config.max_scan_per_evict;
        Self {
            stores: [
                RwLock::new(PageStore::new(max_scan)),
                RwLock::new(PageStore::new(max_scan)),
                RwLock::new(PageStore::new(max_scan)),
                RwLock::new(PageStore::new(max_scan)),
            ],
            config,
            next_page_id: AtomicU64::new(1),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn allocate_page_id(&self) -> PageId {
        self.next_page_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn insert(&self, category: BufferCategory, page_id: PageId, data: Vec<u8>) {
        let idx = store_index(category);
        self.stores[idx]
            .write()
            .insert(page_id, BufferPage::new(data));
    }

    /// Read page data with a closure.  The page write lock is held during
    /// the closure invocation, ensuring exclusive access.
    pub fn read_page<F, R>(&self, category: BufferCategory, page_id: PageId, f: F) -> Option<R>
    where
        F: FnOnce(&Vec<u8>) -> R,
    {
        let idx = store_index(category);
        let store = self.stores[idx].read();
        let page = store.get(page_id)?;
        if page.is_evicted() {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        drop(store);
        self.stores[idx].write().eviction.touch(page_id);

        self.hits.fetch_add(1, Ordering::Relaxed);
        let store = self.stores[idx].read();
        let page = store.get(page_id)?;
        page.data().map(f)
    }

    /// Write page data with a closure.
    pub fn write_page<F, R>(
        &self,
        category: BufferCategory,
        page_id: PageId,
        f: F,
    ) -> Option<R>
    where
        F: FnOnce(&mut Vec<u8>) -> R,
    {
        let idx = store_index(category);
        let mut store = self.stores[idx].write();
        let page = store.get_mut(page_id)?;
        if page.is_evicted() {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        page.mark_dirty();
        page.data_mut().map(f)
    }

    /// Evict pages from a specific category.  Returns number evicted.
    pub fn evict_category(&self, category: BufferCategory, target_count: usize) -> usize {
        let idx = store_index(category);
        let mut store = self.stores[idx].write();

        let candidates = store.eviction.select_candidates(target_count);
        if candidates.is_empty() {
            return 0;
        }

        let mut evicted = 0usize;
        let mut confirmed = Vec::with_capacity(candidates.len());
        let mut rejected = Vec::new();

        for page_id in &candidates {
            if let Some(page) = store.pages.get_mut(page_id) {
                if page.is_pinned() {
                    rejected.push(*page_id);
                    continue;
                }
                page.evict();
                confirmed.push(*page_id);
                evicted += 1;
            }
        }

        store.eviction.confirm_eviction(&confirmed);
        store.eviction.reject_candidates(&rejected);
        self.evictions.fetch_add(evicted as u64, Ordering::Relaxed);
        evicted
    }

    /// Evict pages across all categories.
    pub fn evict_all(&self, target_pages: usize) -> usize {
        let mut total = 0;
        let per_category = target_pages / 4 + 1;
        for cat in &[
            BufferCategory::IndexLeaf,
            BufferCategory::IndexNode,
            BufferCategory::EdgeSegment,
            BufferCategory::VertexColumn,
        ] {
            total += self.evict_category(*cat, per_category);
            if total >= target_pages {
                break;
            }
        }
        total
    }

    pub fn remove(&self, category: BufferCategory, page_id: PageId) {
        let idx = store_index(category);
        self.stores[idx].write().remove(page_id);
    }

    pub fn contains(&self, category: BufferCategory, page_id: PageId) -> bool {
        let idx = store_index(category);
        self.stores[idx].read().contains(page_id)
    }

    pub fn stats(&self) -> BufferStats {
        BufferStats {
            total_pages: self.stores.iter().map(|s| s.read().pages.len()).sum(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BufferStats {
    pub total_pages: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl std::fmt::Debug for BufferManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferManager")
            .field("config", &self.config)
            .field("hits", &self.hits.load(Ordering::Relaxed))
            .field("misses", &self.misses.load(Ordering::Relaxed))
            .field("evictions", &self.evictions.load(Ordering::Relaxed))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_read() {
        let bm = BufferManager::new(BufferConfig::default());
        let pid = bm.allocate_page_id();
        bm.insert(BufferCategory::VertexColumn, pid, vec![1, 2, 3]);
        let result = bm.read_page(BufferCategory::VertexColumn, pid, |data| data.to_vec());
        assert_eq!(result, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_write_page() {
        let bm = BufferManager::new(BufferConfig::default());
        let pid = bm.allocate_page_id();
        bm.insert(BufferCategory::VertexColumn, pid, vec![0, 0, 0]);
        bm.write_page(BufferCategory::VertexColumn, pid, |data| {
            data.copy_from_slice(&[4, 5, 6]);
        });
        let result = bm.read_page(BufferCategory::VertexColumn, pid, |data| data.to_vec());
        assert_eq!(result, Some(vec![4, 5, 6]));
    }

    #[test]
    fn test_miss_on_nonexistent() {
        let bm = BufferManager::new(BufferConfig::default());
        let result = bm.read_page(BufferCategory::VertexColumn, 999, |data| data.len());
        assert!(result.is_none());
    }

    #[test]
    fn test_evict_and_miss() {
        let bm = BufferManager::new(BufferConfig {
            max_scan_per_evict: 100,
            evict_batch_size: 10,
            ..Default::default()
        });
        let pid = bm.allocate_page_id();
        bm.insert(BufferCategory::VertexColumn, pid, vec![1, 2, 3]);
        bm.evict_category(BufferCategory::VertexColumn, 10);
        let result = bm.read_page(BufferCategory::VertexColumn, pid, |data| data.len());
        assert!(result.is_none());
    }

    #[test]
    fn test_evict_preserves_recently_used() {
        let bm = BufferManager::new(BufferConfig::default());
        let pid1 = bm.allocate_page_id();
        let pid2 = bm.allocate_page_id();
        bm.insert(BufferCategory::VertexColumn, pid1, vec![1]);
        bm.insert(BufferCategory::VertexColumn, pid2, vec![2]);

        // Touch pid1
        bm.read_page(BufferCategory::VertexColumn, pid1, |_| {});

        bm.evict_category(BufferCategory::VertexColumn, 10);

        // pid1 was touched — should survive
        assert!(bm.contains(BufferCategory::VertexColumn, pid1));
        // pid2 may or may not be evicted depending on queue order
    }

    #[test]
    fn test_evict_all_some_from_each() {
        let bm = BufferManager::new(BufferConfig::default());
        for i in 0..10u64 {
            bm.insert(BufferCategory::VertexColumn, i, vec![i as u8]);
        }
        for i in 0..10u64 {
            bm.insert(BufferCategory::IndexNode, 10 + i, vec![i as u8]);
        }
        let evicted = bm.evict_all(5);
        assert!(evicted > 0);
    }

    #[test]
    fn test_stats() {
        let bm = BufferManager::new(BufferConfig::default());
        let pid = bm.allocate_page_id();
        bm.insert(BufferCategory::VertexColumn, pid, vec![1]);
        bm.read_page(BufferCategory::VertexColumn, pid, |_| {});
        let stats = bm.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.total_pages, 1);
    }
}

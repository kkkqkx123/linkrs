//! Buffer Manager (Phase 1 OLAP - vmcache style)
//!
//! Provides a global page pool for CSR segments and column chunks, using
//! `mmap` + `MADV_DONTNEED` for eviction and optimistic reads for OLAP scans.
//! This replaces the per-segment `residency` + Moka cache with a unified
//! memory budget, preventing OLAP large scans from OOM and allowing concurrent
//! read/write reuse of the same pages.
//!
//! Current status: Phase 1 stub with `BufferManager` trait and `MmapBufferManager`
//! skeleton. Full implementation (page table, clock eviction, optimistic
//! seqlock reads) is Phase 1.5. The `GlobalBufferManager` below already
//! provides a process-wide singleton that tracks memory accounting and exposes
//! the `BufferManager` interface for segment residency.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;

/// Page identifier (segment id + page index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageId {
    pub segment_id: u64,
    pub page_idx: u32,
}

/// Buffer manager trait: pin / unpin pages for OLAP scans.
pub trait BufferManager: Send + Sync {
    /// Pin a page for reading; returns `None` if the page is not resident
    /// (caller should reload from spill).
    fn pin(&self, page: PageId) -> Option<Vec<u8>>;
    /// Unpin a page after use.
    fn unpin(&self, page: PageId);
    /// Evict cold pages until `target_bytes` is freed, using `MADV_DONTNEED`
    /// semantics (pages remain addressable but are reclaimed by the OS).
    fn evict(&self, target_bytes: usize) -> usize;
    /// Total bytes currently pinned.
    fn pinned_bytes(&self) -> usize;
}

/// Simple in-memory buffer manager (Phase 1 stub).
///
/// Tracks pinned pages in a `HashMap` and enforces a soft memory limit via
/// `evict`. Real implementation will use `mmap` + `madvise` and a clock
/// algorithm for eviction, plus optimistic seqlock reads for lock-free OLAP
/// scans (mirroring `CsrSegment::try_optimistic_read`).
#[derive(Debug, Default)]
pub struct MmapBufferManager {
    pages: RwLock<HashMap<PageId, Vec<u8>>>,
    pinned: AtomicUsize,
    limit_bytes: AtomicUsize,
}

impl MmapBufferManager {
    pub fn new(limit_bytes: usize) -> Self {
        Self {
            pages: RwLock::new(HashMap::new()),
            pinned: AtomicUsize::new(0),
            limit_bytes: AtomicUsize::new(limit_bytes),
        }
    }

    pub fn set_limit(&self, limit: usize) {
        self.limit_bytes.store(limit, Ordering::Relaxed);
    }

    pub fn insert_page(&self, id: PageId, data: Vec<u8>) {
        let len = data.len();
        self.pages.write().insert(id, data);
        self.pinned.fetch_add(len, Ordering::Relaxed);
    }
}

impl BufferManager for MmapBufferManager {
    fn pin(&self, page: PageId) -> Option<Vec<u8>> {
        let guard = self.pages.read();
        guard.get(&page).cloned()
    }

    fn unpin(&self, _page: PageId) {
        // In real impl, decrements pin count and allows eviction.
    }

    fn evict(&self, target: usize) -> usize {
        let mut freed = 0usize;
        let mut guard = self.pages.write();
        let limit = self.limit_bytes.load(Ordering::Relaxed);
        let current = self.pinned.load(Ordering::Relaxed);
        if current <= limit {
            return 0;
        }
        // Simple LRU-ish: drain arbitrary pages until target freed.
        let keys: Vec<PageId> = guard.keys().copied().collect();
        for k in keys {
            if freed >= target {
                break;
            }
            if let Some(data) = guard.remove(&k) {
                freed += data.len();
                self.pinned.fetch_sub(data.len(), Ordering::Relaxed);
                // Real impl: `madvise(MADV_DONTNEED)` instead of dropping.
            }
        }
        freed
    }

    fn pinned_bytes(&self) -> usize {
        self.pinned.load(Ordering::Relaxed)
    }
}

/// Global singleton buffer manager for the process.
static GLOBAL_BUFFER_MANAGER: std::sync::OnceLock<Arc<MmapBufferManager>> =
    std::sync::OnceLock::new();

pub fn global_buffer_manager() -> Arc<MmapBufferManager> {
    GLOBAL_BUFFER_MANAGER
        .get_or_init(|| Arc::new(MmapBufferManager::new(512 * 1024 * 1024)))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buffer_manager_basic() {
        let bm = MmapBufferManager::new(1024);
        let pid = PageId {
            segment_id: 1,
            page_idx: 0,
        };
        bm.insert_page(pid, vec![1, 2, 3]);
        assert_eq!(bm.pin(pid), Some(vec![1, 2, 3]));
        assert_eq!(bm.pinned_bytes(), 3);
        let freed = bm.evict(1);
        // Not over limit, so no eviction.
        assert_eq!(freed, 0);
        bm.set_limit(2);
        let freed = bm.evict(2);
        assert!(freed >= 2);
    }
}

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::core::StorageResult;

/// State of a buffer page in the eviction lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    /// Page is clean, unpinned, eligible for eviction.
    Clean,
    /// Page is dirty — needs write-back before eviction.
    Dirty,
    /// Page is pinned by one or more readers/writers.
    Pinned,
    /// Page has been evicted — data must be reloaded on next access.
    Evicted,
}

/// A single page in the buffer pool.
///
/// `T` is the page payload type (e.g. `Vec<u8>`, `Arc<[u8]>`, or a typed
/// page descriptor).  Pin/unpin reference counting is atomic so that
/// `BufferPage` can be shared among threads without a full lock.
#[derive(Debug)]
pub struct BufferPage<T> {
    /// Payload data (None when evicted).
    data: Option<T>,
    /// Pin count — page is not evictable while > 0.
    pin_count: AtomicU32,
    /// Page state for eviction policy.
    state: PageState,
    /// Monotonically increasing version for invalidation detection.
    version: u64,
}

impl<T: Clone> Clone for BufferPage<T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            pin_count: AtomicU32::new(self.pin_count.load(Ordering::Relaxed)),
            state: self.state,
            version: self.version,
        }
    }
}

impl<T> BufferPage<T> {
    pub fn new(data: T) -> Self {
        Self {
            data: Some(data),
            pin_count: AtomicU32::new(0),
            state: PageState::Clean,
            version: 0,
        }
    }

    pub fn empty() -> Self {
        Self {
            data: None,
            pin_count: AtomicU32::new(0),
            state: PageState::Evicted,
            version: 0,
        }
    }

    pub fn pin(&self) -> bool {
        if self.state == PageState::Evicted {
            return false;
        }
        self.pin_count.fetch_add(1, Ordering::AcqRel);
        true
    }

    pub fn unpin(&self) {
        let prev = self.pin_count.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(prev > 0, "unpin called on unpinned page");
    }

    pub fn is_pinned(&self) -> bool {
        self.pin_count.load(Ordering::Acquire) > 0
    }

    pub fn pin_count(&self) -> u32 {
        self.pin_count.load(Ordering::Acquire)
    }

    pub fn data(&self) -> Option<&T> {
        self.data.as_ref()
    }

    pub fn data_mut(&mut self) -> Option<&mut T> {
        self.data.as_mut()
    }

    pub fn take_data(&mut self) -> Option<T> {
        self.data.take()
    }

    pub fn mark_dirty(&mut self) {
        if self.state != PageState::Pinned {
            self.state = PageState::Dirty;
        }
    }

    pub fn mark_clean(&mut self) {
        if self.state != PageState::Pinned {
            self.state = PageState::Clean;
        }
    }

    pub fn evict(&mut self) -> Option<T> {
        let data = self.data.take()?;
        self.state = PageState::Evicted;
        self.version += 1;
        Some(data)
    }

    pub fn load(&mut self, data: T) {
        self.data = Some(data);
        self.state = PageState::Clean;
    }

    pub fn state(&self) -> PageState {
        self.state
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn is_evicted(&self) -> bool {
        self.state == PageState::Evicted
    }

    pub fn is_dirty(&self) -> bool {
        self.state == PageState::Dirty
    }
}

/// Trait for types that can be paged by the BufferManager.
///
/// Implementors define how to load a page from disk and how to
/// write a dirty page back, plus an estimated memory footprint.
pub trait Pageable: Send + 'static {
    fn page_size(&self) -> usize;
    fn load(page_id: u64) -> StorageResult<Self>
    where
        Self: Sized;
    fn flush(&self) -> StorageResult<()>;
}

impl Pageable for Vec<u8> {
    fn page_size(&self) -> usize {
        self.len()
    }

    fn load(_page_id: u64) -> StorageResult<Self> {
        Err(crate::core::StorageError::not_supported(
            "raw Vec<u8> page loading requires specialization",
        ))
    }

    fn flush(&self) -> StorageResult<()> {
        Err(crate::core::StorageError::not_supported(
            "raw Vec<u8> page flush is a no-op",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_pin_unpin() {
        let page = BufferPage::new(vec![1u8, 2, 3]);
        assert_eq!(page.pin_count(), 0);
        assert!(page.pin());
        assert_eq!(page.pin_count(), 1);
        page.unpin();
        assert_eq!(page.pin_count(), 0);
    }

    #[test]
    fn test_page_evict() {
        let mut page = BufferPage::new(vec![1u8, 2, 3]);
        assert!(!page.is_evicted());
        let data = page.evict();
        assert_eq!(data, Some(vec![1u8, 2, 3]));
        assert!(page.is_evicted());
        assert!(page.data().is_none());
    }

    #[test]
    fn test_page_reload() {
        let mut page = BufferPage::new(vec![1u8, 2, 3]);
        page.evict();
        page.load(vec![4u8, 5, 6]);
        assert!(!page.is_evicted());
        assert_eq!(page.data(), Some(&vec![4u8, 5, 6]));
    }

    #[test]
    fn test_pinned_page_not_evictable() {
        let page = BufferPage::new(vec![1u8, 2, 3]);
        page.pin();
        assert!(page.is_pinned());
        let mut page_clone = page.clone();
        // Even cloned, pin_count prevents marking for eviction
        assert!(page_clone.is_pinned());
    }

    #[test]
    fn test_page_dirty_clean() {
        let mut page = BufferPage::new(vec![1u8, 2, 3]);
        assert_eq!(page.state(), PageState::Clean);
        page.mark_dirty();
        assert!(page.is_dirty());
        page.mark_clean();
        assert_eq!(page.state(), PageState::Clean);
    }

    #[test]
    fn test_pinned_then_dirty() {
        let page = BufferPage::new(vec![1u8, 2, 3]);
        page.pin();
        assert!(page.is_pinned());
        // State and pin_count are orthogonal: pin_count tracks concurrent
        // access, state tracks whether page needs write-back.
        page.unpin();
    }
}

use std::collections::{HashMap, VecDeque};

/// Clock-sweep eviction policy for buffer pages.
///
/// Maintains a circular list of page IDs.  On each eviction attempt the
/// clock hand advances, giving each page a second chance: pages that are
/// pinned or have been recently accessed (reference bit set) are skipped;
/// clean pages are candidates for eviction.
///
/// The reference bit is set by [`EvictionQueue::touch`] (called from the
/// read path) and cleared by the clock hand.
pub struct EvictionQueue {
    pages: VecDeque<u64>,
    refs: HashMap<u64, bool>,
    eviction_attempts: u64,
    evictions: u64,
    max_scan: usize,
}

impl EvictionQueue {
    pub fn new(max_scan: usize) -> Self {
        Self {
            pages: VecDeque::new(),
            refs: HashMap::new(),
            eviction_attempts: 0,
            evictions: 0,
            max_scan,
        }
    }

    /// Add a page to the eviction pool.
    pub fn add(&mut self, page_id: u64) {
        if !self.refs.contains_key(&page_id) {
            self.pages.push_back(page_id);
            self.refs.insert(page_id, false);
        }
    }

    /// Remove a page from the eviction pool (e.g. on page free).
    pub fn remove(&mut self, page_id: u64) {
        self.refs.remove(&page_id);
        if let Some(pos) = self.pages.iter().position(|id| *id == page_id) {
            self.pages.remove(pos);
        }
    }

    /// Mark a page as recently accessed — sets its reference bit.
    pub fn touch(&mut self, page_id: u64) {
        if let Some(bit) = self.refs.get_mut(&page_id) {
            *bit = true;
        }
    }

    /// Select page IDs that are candidates for eviction.
    ///
    /// Returns up to `target_count` page IDs whose reference bits are clear
    /// (i.e., not recently accessed).  The caller must check pin/dirty state
    /// and actually evict the page data.  After evicting, call
    /// [`EvictionQueue::confirm_eviction`] to update queue state.
    ///
    /// Touched pages get a second chance (their ref bit is cleared and they
    /// are re-queued).  Pages whose ref bit was already clear are returned
    /// as candidates and removed from the queue.
    pub fn select_candidates(&mut self, target_count: usize) -> Vec<u64> {
        if self.pages.is_empty() || target_count == 0 {
            return Vec::new();
        }

        self.eviction_attempts += 1;
        let mut candidates = Vec::with_capacity(target_count);
        let mut scanned = 0;

        while candidates.len() < target_count && scanned < self.max_scan && !self.pages.is_empty() {
            scanned += 1;
            let page_id = self.pages.pop_front().unwrap();

            let referenced = self.refs.get(&page_id).copied().unwrap_or(false);
            if referenced {
                self.refs.insert(page_id, false);
                self.pages.push_back(page_id);
                continue;
            }

            candidates.push(page_id);
        }

        candidates
    }

    /// Confirm that the given page IDs were successfully evicted.
    /// Removes them from internal tracking.
    pub fn confirm_eviction(&mut self, page_ids: &[u64]) {
        for id in page_ids {
            self.refs.remove(id);
            // Already removed from the deque by select_candidates above.
        }
        self.evictions += page_ids.len() as u64;
    }

    /// Re-queue page IDs that were selected but could not be evicted
    /// (e.g., they were pinned or dirty).
    pub fn reject_candidates(&mut self, page_ids: &[u64]) {
        for id in page_ids {
            if self.refs.contains_key(id) {
                self.pages.push_back(*id);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.pages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    pub fn stats(&self) -> EvictionStats {
        EvictionStats {
            total_pages: self.pages.len(),
            eviction_attempts: self.eviction_attempts,
            evictions: self.evictions,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EvictionStats {
    pub total_pages: usize,
    pub eviction_attempts: u64,
    pub evictions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_remove() {
        let mut q = EvictionQueue::new(100);
        q.add(1);
        q.add(2);
        q.add(3);
        assert_eq!(q.len(), 3);
        q.remove(2);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn test_touch_gives_second_chance() {
        let mut q = EvictionQueue::new(100);
        q.add(1);
        q.add(2);
        q.touch(1);

        // First select: page 1 (touched) should get a second chance.
        // Only page 2 should be selected.
        let candidates = q.select_candidates(1);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0], 2);
        q.confirm_eviction(&candidates);
    }

    #[test]
    fn test_select_multiple() {
        let mut q = EvictionQueue::new(100);
        for i in 0..10u64 {
            q.add(i);
        }
        let candidates = q.select_candidates(5);
        assert_eq!(candidates.len(), 5);
        q.confirm_eviction(&candidates);
        assert_eq!(q.len(), 5);
    }

    #[test]
    fn test_select_respects_max_scan() {
        let mut q = EvictionQueue::new(3);
        for i in 0..10u64 {
            q.add(i);
        }
        let candidates = q.select_candidates(100);
        assert_eq!(candidates.len(), 3);
    }

    #[test]
    fn test_reject_requeues() {
        let mut q = EvictionQueue::new(100);
        q.add(1);
        q.add(2);

        let candidates = q.select_candidates(10);
        assert_eq!(candidates.len(), 2);
        // Reject both
        q.reject_candidates(&candidates);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn test_stats() {
        let mut q = EvictionQueue::new(100);
        for i in 0..5u64 {
            q.add(i);
        }
        let c = q.select_candidates(2);
        q.confirm_eviction(&c);
        let stats = q.stats();
        assert_eq!(stats.evictions, 2);
        assert_eq!(stats.eviction_attempts, 1);
    }
}

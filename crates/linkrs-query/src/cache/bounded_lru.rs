//! Small bounded move-to-front lookup table.
//!
//! Several query caches keep only a handful of entries on a hot path. This
//! component owns their storage and eviction; cache-specific keys, counters
//! and invalidation policies stay with the owning cache.

use parking_lot::Mutex;
use std::borrow::Borrow;

/// A bounded map backed by a linear scan with move-to-front reuse.
pub(crate) struct BoundedLru<K, V> {
    entries: Mutex<Vec<(K, V)>>,
    capacity: usize,
}

impl<K: PartialEq, V: Clone> BoundedLru<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            capacity,
        }
    }

    /// Probe without changing entry order.
    pub(crate) fn contains<Q: ?Sized>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: PartialEq,
    {
        self.entries
            .lock()
            .iter()
            .any(|(entry, _)| entry.borrow() == key)
    }

    /// Look up a value and move its entry to the front.
    pub(crate) fn get<Q: ?Sized>(&self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: PartialEq,
    {
        let mut guard = self.entries.lock();
        let position = guard.iter().position(|(entry, _)| entry.borrow() == key)?;
        if position > 0 {
            let entry = guard.remove(position);
            guard.insert(0, entry);
        }
        Some(guard[0].1.clone())
    }

    /// Replace a same-key entry, then evict the least recently used entry
    /// past the capacity.
    pub(crate) fn store(&self, key: K, value: V) {
        let mut guard = self.entries.lock();
        guard.retain(|(entry, _)| entry != &key);
        guard.insert(0, (key, value));
        guard.truncate(self.capacity);
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub(crate) fn clear(&self) {
        self.entries.lock().clear();
    }
}

use std::collections::HashSet;

use super::super::Timestamp;
use super::MutableCsr;

/// Live keys at or below this count stay in a sorted inline array with
/// binary search; larger rows upgrade to a hash set. Covers the common
/// small-degree case without per-edge heap entries or hashing.
pub(crate) const LIVE_SET_SORTED_BOUND: usize = 8;

/// Live endpoint keys of one vertex, tiered by width.
///
/// Small rows hold a sorted inline array; rows growing past
/// [`LIVE_SET_SORTED_BOUND`] upgrade to a hash set. Upgrades never
/// downgrade on removal so widths oscillating around the bound do not flap
/// between representations; rebuilds reselect by current width.
#[derive(Debug, Clone)]
pub(crate) enum LiveKeySet {
    Sorted(Vec<(u32, i64)>),
    Hashed(HashSet<(u32, i64)>),
}

impl Default for LiveKeySet {
    fn default() -> Self {
        LiveKeySet::Sorted(Vec::new())
    }
}

impl LiveKeySet {
    fn from_keys(mut keys: Vec<(u32, i64)>) -> Self {
        if keys.len() <= LIVE_SET_SORTED_BOUND {
            keys.sort_unstable();
            keys.dedup();
            LiveKeySet::Sorted(keys)
        } else {
            LiveKeySet::Hashed(keys.into_iter().collect())
        }
    }

    pub(crate) fn contains(&self, key: &(u32, i64)) -> bool {
        match self {
            LiveKeySet::Sorted(keys) => keys.binary_search(key).is_ok(),
            LiveKeySet::Hashed(set) => set.contains(key),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            LiveKeySet::Sorted(keys) => keys.len(),
            LiveKeySet::Hashed(set) => set.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn insert(&mut self, key: (u32, i64)) {
        match self {
            LiveKeySet::Sorted(keys) => {
                if keys.binary_search(&key).is_err() {
                    keys.push(key);
                    if keys.len() > LIVE_SET_SORTED_BOUND {
                        let upgraded: HashSet<(u32, i64)> =
                            std::mem::take(keys).into_iter().collect();
                        *self = LiveKeySet::Hashed(upgraded);
                    } else {
                        keys.sort_unstable();
                    }
                }
            }
            LiveKeySet::Hashed(set) => {
                set.insert(key);
            }
        }
    }

    pub(crate) fn remove(&mut self, key: &(u32, i64)) {
        match self {
            LiveKeySet::Sorted(keys) => {
                if let Ok(pos) = keys.binary_search(key) {
                    keys.remove(pos);
                }
            }
            LiveKeySet::Hashed(set) => {
                set.remove(key);
            }
        }
    }

    /// Heap bytes held outside the enum itself, following the memory
    /// estimate caliber used by the row statistics.
    pub(crate) fn heap_bytes(&self) -> usize {
        match self {
            LiveKeySet::Sorted(keys) => keys.capacity() * std::mem::size_of::<(u32, i64)>(),
            LiveKeySet::Hashed(set) => set.len() * (std::mem::size_of::<(u32, i64)>() + 8),
        }
    }
}

impl MutableCsr {
    pub(crate) fn rebuild_live_sets(&mut self) {
        let capacity = self.vertex_capacity() as u32;
        self.live_sets.clear();
        for vid in 0..capacity {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    /// Whether one vertex currently holds a live entry for `key`.
    pub(crate) fn live_key_present(&self, vid: u32, endpoint: u32, rank: i64) -> bool {
        self.live_sets
            .get(&vid)
            .is_some_and(|set| set.contains(&(endpoint, rank)))
    }

    /// Live key count of one vertex, zero when the vertex holds no entry.
    pub(crate) fn live_key_count(&self, vid: u32) -> usize {
        self.live_sets.get(&vid).map_or(0, LiveKeySet::len)
    }

    pub(crate) fn track_live_insert(&mut self, vid: u32, endpoint: u32, rank: i64) {
        self.live_sets
            .entry(vid)
            .or_default()
            .insert((endpoint, rank));
    }

    pub(crate) fn track_live_remove(&mut self, vid: u32, endpoint: u32, rank: i64) {
        if let Some(set) = self.live_sets.get_mut(&vid) {
            set.remove(&(endpoint, rank));
            if set.is_empty() {
                self.live_sets.remove(&vid);
            }
        }
    }

    pub(crate) fn rebuild_live_set_for_vertex(&mut self, vid: u32) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.live_sets.remove(&vid);
            return;
        }
        let degree = self.degrees[idx] as usize;
        let offset = self.adj_offsets[idx] as usize;
        let mut keys = Vec::new();
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    keys.push((nbr.endpoint, nbr.rank));
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        keys.push((nbr.endpoint, nbr.rank));
                    }
                }
            }
        }
        if keys.is_empty() {
            self.live_sets.remove(&vid);
        } else {
            self.live_sets.insert(vid, LiveKeySet::from_keys(keys));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_widths_stay_sorted() {
        let mut set = LiveKeySet::from_keys(vec![(3, 0), (1, 0), (2, 0)]);
        assert!(matches!(set, LiveKeySet::Sorted(_)));
        assert!(set.contains(&(1, 0)));
        assert!(!set.contains(&(9, 0)));
        set.remove(&(2, 0));
        assert!(!set.contains(&(2, 0)));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn growth_past_bound_upgrades_to_hashed() {
        let mut set = LiveKeySet::default();
        for i in 0..=(LIVE_SET_SORTED_BOUND as u32) {
            set.insert((i, 0));
        }
        assert!(matches!(set, LiveKeySet::Hashed(_)));
        for i in 0..=(LIVE_SET_SORTED_BOUND as u32) {
            assert!(set.contains(&(i, 0)));
        }
        // Removal past the upgrade never downgrades; behavior stays exact.
        set.remove(&(0, 0));
        assert!(matches!(set, LiveKeySet::Hashed(_)));
        assert!(!set.contains(&(0, 0)));
    }

    #[test]
    fn duplicate_inserts_stay_unique() {
        let mut set = LiveKeySet::default();
        set.insert((1, 5));
        set.insert((1, 5));
        assert_eq!(set.len(), 1);
    }
}

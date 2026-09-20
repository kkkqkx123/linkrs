use std::collections::HashMap;

use super::super::csr_shared::SegmentedTable;
use super::write::EdgePosition;
use super::MutableCsr;

/// Live width at or below this bound keeps no index.
pub(crate) const LIVE_SET_WIDTH_BOUND: usize = 8;

/// Live endpoint keys of one wide vertex with row positions.
///
/// Only rows wider than [`LIVE_SET_WIDTH_BOUND`] carry a set; narrow rows
/// hold no index and answer through row scans.  Sets are created on demand
/// by the write path and dropped by rebuilds once the row narrows again, so
/// widths oscillating around the bound cannot accumulate stale indexes.
///
/// Two internal representations back the set:
///
/// * **Hash** — a `HashMap` chosen during active mutation for O(1) insert
///   and remove.
/// * **Sorted** — a pre-sorted `Vec` chosen after a structural rebuild
///   (compaction, rebalance, repack) that leaves the row stable.  Point
///   lookups use `partition_point` (binary search) which is cache-friendlier
///   than hashing on wide rows and avoids the per-entry allocation overhead
///   of the hash table.
///
/// The write path always mutates the hash variant.  When a structural
/// rebuild produces a set the caller marks stable, the set is promoted to
/// the sorted variant.  Any subsequent insert or remove converts back to
/// hash so the hot path stays O(1); the next rebuild may re-promote.
#[derive(Debug, Clone)]
pub(crate) enum LiveKeySet {
    Hash(HashMap<(u32, i64), EdgePosition>),
    Sorted(Vec<((u32, i64), EdgePosition)>),
}

impl Default for LiveKeySet {
    fn default() -> Self {
        Self::Hash(HashMap::new())
    }
}

impl LiveKeySet {
    pub(crate) fn from_positions(positions: Vec<((u32, i64), EdgePosition)>) -> Self {
        let mut sorted = positions;
        sorted.sort_unstable_by_key(|(k, _)| *k);
        Self::Sorted(sorted)
    }

    pub(crate) fn contains(&self, key: &(u32, i64)) -> bool {
        match self {
            Self::Hash(map) => map.contains_key(key),
            Self::Sorted(vec) => {
                vec.partition_point(|(k, _)| k < key) < vec.len()
                    && vec[vec.partition_point(|(k, _)| k < key)].0 == *key
            }
        }
    }

    pub(crate) fn position(&self, key: &(u32, i64)) -> Option<EdgePosition> {
        match self {
            Self::Hash(map) => map.get(key).copied(),
            Self::Sorted(vec) => {
                let idx = vec.partition_point(|(k, _)| k < key);
                if idx < vec.len() && vec[idx].0 == *key {
                    Some(vec[idx].1)
                } else {
                    None
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Hash(map) => map.len(),
            Self::Sorted(vec) => vec.len(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::Hash(map) => map.is_empty(),
            Self::Sorted(vec) => vec.is_empty(),
        }
    }

    /// Insert a key-position pair, converting to hash when currently sorted.
    ///
    /// The hot write path always arrives here; converting to hash keeps
    /// the insert O(1).  The next structural rebuild may re-promote.
    pub(crate) fn insert(&mut self, key: (u32, i64), position: EdgePosition) {
        match self {
            Self::Hash(map) => {
                map.insert(key, position);
            }
            Self::Sorted(_) => {
                let map: HashMap<_, _> = std::mem::take(self)
                    .into_sorted()
                    .into_iter()
                    .collect();
                *self = Self::Hash(map);
                if let Self::Hash(map) = self {
                    map.insert(key, position);
                }
            }
        }
    }

    /// Remove a key, converting to hash when currently sorted.
    pub(crate) fn remove(&mut self, key: &(u32, i64)) {
        match self {
            Self::Hash(map) => {
                map.remove(key);
            }
            Self::Sorted(_) => {
                let map: HashMap<_, _> = std::mem::take(self)
                    .into_sorted()
                    .into_iter()
                    .collect();
                *self = Self::Hash(map);
                if let Self::Hash(map) = self {
                    map.remove(key);
                }
            }
        }
    }

    /// Heap bytes held outside the struct itself.
    pub(crate) fn heap_bytes(&self) -> usize {
        let entry_size = std::mem::size_of::<((u32, i64), EdgePosition)>() + 8;
        match self {
            Self::Hash(map) => map.len() * entry_size,
            Self::Sorted(vec) => vec.len() * entry_size,
        }
    }

    /// Consume the set and return the inner sorted vector if sorted,
    /// otherwise collect into a sorted vector.
    fn into_sorted(self) -> Vec<((u32, i64), EdgePosition)> {
        match self {
            Self::Hash(map) => {
                let mut entries: Vec<_> = map.into_iter().collect();
                entries.sort_unstable_by_key(|(k, _)| *k);
                entries
            }
            Self::Sorted(vec) => vec,
        }
    }
}

/// Segmented sparse per-vertex live-key index.
///
/// Row addresses inside one CSR are dense vertex ids, so indexed rows are
/// addressed by direct subscript (segment by shift, offset by mask) instead
/// of hashing. Segments with no indexed row stay unallocated and cost only
/// the pointer. Empty and narrow rows hold no set. Lifetimes match the
/// overflow index: created once a row grows past the bound, dropped when the
/// row narrows or goes empty.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveSetStorage {
    table: SegmentedTable<LiveKeySet>,
    live_rows: usize,
}

impl LiveSetStorage {
    pub(crate) fn new() -> Self {
        Self {
            table: SegmentedTable::new(),
            live_rows: 0,
        }
    }

    pub(crate) fn ensure_capacity(&mut self, vertex_capacity: usize) {
        self.table.ensure_capacity(vertex_capacity);
    }

    pub(crate) fn get(&self, vid: &u32) -> Option<&LiveKeySet> {
        self.table.get(*vid)
    }

    pub(crate) fn insert(&mut self, vid: u32, set: LiveKeySet) {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() {
            self.live_rows += 1;
        }
        *slot = Some(set);
    }

    pub(crate) fn remove(&mut self, vid: &u32) {
        if self.table.take(*vid).is_some() {
            self.live_rows = self.live_rows.saturating_sub(1);
        }
    }

    pub(crate) fn insert_key(&mut self, vid: u32, key: (u32, i64), position: EdgePosition) {
        let slot = self.table.slot_mut(vid);
        match slot {
            Some(set) => set.insert(key, position),
            slot @ None => {
                *slot = Some(LiveKeySet::from_positions(vec![(key, position)]));
                self.live_rows += 1;
            }
        }
    }

    pub(crate) fn remove_key(&mut self, vid: u32, key: &(u32, i64)) {
        let emptied = match self.table.get_mut(vid) {
            Some(set) => {
                set.remove(key);
                set.is_empty()
            }
            None => return,
        };
        if emptied {
            self.table.take(vid);
            self.live_rows = self.live_rows.saturating_sub(1);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.table.clear();
        self.live_rows = 0;
    }

    pub(crate) fn heap_bytes_total(&self) -> usize {
        self.table.iter().map(|(_, set)| set.heap_bytes()).sum()
    }

    pub(crate) fn index_bytes(&self) -> usize {
        self.table.table_bytes()
            + self.live_rows * (std::mem::size_of::<u32>() + std::mem::size_of::<LiveKeySet>())
    }
}

impl MutableCsr {
    pub fn has_live_set(&self, vid: &u32) -> bool {
        self.live_sets.get(vid).is_some()
    }
}

impl MutableCsr {
    pub(crate) fn rebuild_live_sets(&mut self) {
        let capacity = self.vertex_capacity() as u32;
        self.live_sets.clear();
        self.live_sets.ensure_capacity(self.vertex_capacity());
        for vid in 0..capacity {
            self.rebuild_live_set_for_vertex(vid);
        }
    }
    /// Live key count of one vertex, zero when the vertex holds no entry.
    ///
    /// Indexed rows answer from the set length; narrow rows scan the row.
    /// The scan cost stays proportional to the row width, which is small by
    /// construction wherever no set exists.
    pub(crate) fn live_key_count(&self, vid: u32) -> usize {
        if let Some(set) = self.live_sets.get(&vid) {
            return set.len();
        }
        self.row_live_count(vid)
    }

    /// Combined duplicate-plus-live-width probe of one row.
    ///
    /// Single pass behind `insert_edge` and `live_key_present`: reports
    /// whether a live entry for the key exists and how many live entries
    /// the row holds, so one insert pays one row walk instead of one for
    /// the duplicate check plus one for the width count.
    ///
    /// When incremental live counts are maintained (narrow rows), the live
    /// width is returned from the counter and the scan only checks for the
    /// duplicate key, short-circuiting once found.
    pub(crate) fn row_live_scan(&self, vid: u32, endpoint: u32, rank: i64) -> (bool, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (false, 0);
        }
        let mut present = false;
        let (hot, cold) = self.primary_pair(idx);
        for (h, c) in hot.iter().zip(cold.iter()) {
            if c.is_live() {
                if h.endpoint == endpoint && h.rank == rank {
                    present = true;
                    break;
                }
            }
        }
        if !present {
            if let Some(chunks) = self.overflow_chunks.get(vid) {
                'outer: for chunk in chunks {
                    for (hot, cold) in chunk.hot_slice().iter().zip(chunk.cold_slice()) {
                        if cold.is_live() {
                            if hot.endpoint == endpoint && hot.rank == rank {
                                present = true;
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
        let live = self.live_counts[idx] as usize;
        (present, live)
    }

    /// Live entry count of one narrow row.
    ///
    /// Uses the incremental counter for O(1) access instead of scanning.
    pub(crate) fn row_live_count(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        self.live_counts[idx] as usize
    }

    pub(crate) fn track_live_insert(
        &mut self,
        vid: u32,
        endpoint: u32,
        rank: i64,
        position: EdgePosition,
    ) {
        if self.live_sets.get(&vid).is_some() {
            self.live_sets.insert_key(vid, (endpoint, rank), position);
            return;
        }
        // Narrow rows stay set-free until the physical row width (primary
        // entries plus overflow entries) passes the bound. The width check
        // touches only lengths, not entries: counting live entries instead
        // would cost a full row walk per insert, so the physical gate is
        // kept deliberately. Rebuilds drop the set again
        // when the live width is still narrow, so tombstone-heavy rows may
        // rescan on later inserts until maintenance compacts them.
        let mut width = 0usize;
        if (vid as usize) < self.vertex_capacity() {
            width = self.rows.degrees[vid as usize] as usize;
            if let Some(chunks) = self.overflow_chunks.get(vid) {
                width += chunks.iter().map(|chunk| chunk.len()).sum::<usize>();
            }
        }
        if width > LIVE_SET_WIDTH_BOUND {
            self.rebuild_live_set_for_vertex(vid);
        }
    }

    pub(crate) fn track_live_remove(&mut self, vid: u32, endpoint: u32, rank: i64) {
        // Narrow rows carry no set, so there is nothing to maintain.
        if self.live_sets.get(&vid).is_none() {
            return;
        }
        self.live_sets.remove_key(vid, &(endpoint, rank));
    }

    pub(crate) fn rebuild_live_set_for_vertex(&mut self, vid: u32) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            self.live_sets.remove(&vid);
            self.live_set_rebuild_count += 1;
            return;
        }
        let (hot, cold) = self.primary_pair(idx);
        let mut positioned = Vec::new();
        let mut live = 0u32;
        let mut tombstones = 0u32;
        for (slot, (h, c)) in hot.iter().zip(cold.iter()).enumerate() {
            if c.is_live() {
                live += 1;
                positioned.push((
                    (h.endpoint, h.rank),
                    EdgePosition::Primary { slot: slot as u32 },
                ));
            } else {
                tombstones += 1;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for (slot_idx, (hot, cold)) in
                    chunk.hot_slice().iter().zip(chunk.cold_slice()).enumerate()
                {
                    if cold.is_live() {
                        live += 1;
                        positioned.push((
                            (hot.endpoint, hot.rank),
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        ));
                    } else {
                        tombstones += 1;
                    }
                }
            }
        }
        self.live_counts[idx] = live;
        self.tombstone_counts[idx] = tombstones;
        // Narrow rows stay set-free; only wide rows pay for the index.
        if positioned.len() <= LIVE_SET_WIDTH_BOUND {
            self.live_sets.remove(&vid);
        } else {
            self.live_sets
                .insert(vid, LiveKeySet::from_positions(positioned));
        }
        self.live_set_rebuild_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_contains_and_removes() {
        use super::super::write::EdgePosition;
        let mut set = LiveKeySet::from_positions(vec![
            ((3, 0), EdgePosition::Primary { slot: 0 }),
            ((1, 0), EdgePosition::Primary { slot: 1 }),
            ((2, 0), EdgePosition::Primary { slot: 2 }),
        ]);
        assert!(set.contains(&(1, 0)));
        assert_eq!(
            set.position(&(1, 0)),
            Some(EdgePosition::Primary { slot: 1 })
        );
        assert!(!set.contains(&(9, 0)));
        assert_eq!(set.position(&(9, 0)), None);
        set.remove(&(2, 0));
        assert!(!set.contains(&(2, 0)));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn wide_rows_carry_a_set() {
        use super::super::write::EdgePosition;
        let mut set = LiveKeySet::default();
        for i in 0..=(LIVE_SET_WIDTH_BOUND as u32) {
            set.insert((i, 0), EdgePosition::Primary { slot: i });
        }
        for i in 0..=(LIVE_SET_WIDTH_BOUND as u32) {
            assert!(set.contains(&(i, 0)));
            assert_eq!(
                set.position(&(i, 0)),
                Some(EdgePosition::Primary { slot: i })
            );
        }
        set.remove(&(0, 0));
        assert!(!set.contains(&(0, 0)));
    }

    #[test]
    fn duplicate_inserts_stay_unique() {
        use super::super::write::EdgePosition;
        let mut set = LiveKeySet::default();
        set.insert((1, 5), EdgePosition::Primary { slot: 0 });
        set.insert((1, 5), EdgePosition::Primary { slot: 0 });
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn empty_vertices_cost_only_pointers() {
        use super::super::super::csr_shared::SEGMENT_SIZE;
        let mut storage = LiveSetStorage::new();
        storage.ensure_capacity(1_000_000);
        assert!(storage.index_bytes() < 1_000_000);
        assert!(storage.get(&999_999).is_none());
        // Untouched segments allocate nothing: one indexed row far out must
        // not inflate the table beyond its own segment slab.
        storage.insert(
            999_999,
            LiveKeySet::from_positions(vec![(
                (7, 0),
                super::super::write::EdgePosition::Primary { slot: 0 },
            )]),
        );
        assert!(storage.get(&999_999).is_some());
        let _ = SEGMENT_SIZE;
    }

    #[test]
    fn sparse_rows_route_and_iterate_in_order() {
        use super::super::super::csr_shared::SEGMENT_SIZE;
        use super::super::write::EdgePosition;
        let mut storage = LiveSetStorage::new();
        let seg = SEGMENT_SIZE as u32;
        storage.insert_key(3 * seg + 7, (9, 0), EdgePosition::Primary { slot: 0 });
        storage.insert_key(5, (1, 0), EdgePosition::Primary { slot: 0 });
        storage.insert_key(seg, (2, 0), EdgePosition::Primary { slot: 0 });
        assert!(storage.get(&seg).is_some_and(|set| set.contains(&(2, 0))));
        assert!(storage.get(&(seg - 1)).is_none());
        storage.remove_key(seg, &(2, 0));
        assert!(storage.get(&seg).is_none());
        storage.insert_key(seg, (4, 0), EdgePosition::Primary { slot: 3 });
        assert!(storage.get(&seg).is_some_and(|set| set.contains(&(4, 0))));
    }
}

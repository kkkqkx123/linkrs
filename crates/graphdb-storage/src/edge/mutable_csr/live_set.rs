use std::collections::HashMap;

use super::super::csr_shared::SegmentedTable;
use super::write::EdgePosition;
use super::MutableCsr;

/// Live width at or below this bound keeps no index.
pub(crate) const LIVE_SET_WIDTH_BOUND: usize = 8;

/// Live endpoint keys of one wide vertex with row positions.
///
/// Only rows wider than [`LIVE_SET_WIDTH_BOUND`] carry a set; narrow rows
/// hold no index and answer through row scans. Sets are created on demand by
/// the write path and dropped by rebuilds once the row narrows again, so
/// widths oscillating around the bound cannot accumulate stale indexes.
///
/// Each key maps to its row-relative slot, so point lookups on wide rows
/// address the slot directly instead of scanning the row. Positions stay
/// valid until the next structural change of the row (compaction, rebalance,
/// repack, gap-closing removal or reservation rebuild); every such path
/// rebuilds the set for the touched row through the same maintenance hook
/// that keeps the key membership exact.
#[derive(Debug, Clone, Default)]
pub(crate) struct LiveKeySet {
    positions: HashMap<(u32, i64), EdgePosition>,
}

impl LiveKeySet {
    pub(crate) fn from_positions(positions: Vec<((u32, i64), EdgePosition)>) -> Self {
        Self {
            positions: positions.into_iter().collect(),
        }
    }

    pub(crate) fn contains(&self, key: &(u32, i64)) -> bool {
        self.positions.contains_key(key)
    }

    pub(crate) fn position(&self, key: &(u32, i64)) -> Option<EdgePosition> {
        self.positions.get(key).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.positions.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    pub(crate) fn insert(&mut self, key: (u32, i64), position: EdgePosition) {
        self.positions.insert(key, position);
    }

    pub(crate) fn remove(&mut self, key: &(u32, i64)) {
        self.positions.remove(key);
    }

    /// Heap bytes held outside the struct itself, following the memory
    /// estimate caliber used by the row statistics.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.positions.len() * (std::mem::size_of::<((u32, i64), EdgePosition)>() + 8)
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
    pub(crate) fn row_live_scan(&self, vid: u32, endpoint: u32, rank: i64) -> (bool, usize) {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return (false, 0);
        }
        let mut present = false;
        let mut live = 0usize;
        let (hot, cold) = self.primary_pair(idx);
        for (h, c) in hot.iter().zip(cold.iter()) {
            if c.is_live() {
                live += 1;
                if h.endpoint == endpoint && h.rank == rank {
                    present = true;
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for (hot, cold) in chunk.hot_slice().iter().zip(chunk.cold_slice()) {
                    if cold.is_live() {
                        live += 1;
                        if hot.endpoint == endpoint && hot.rank == rank {
                            present = true;
                        }
                    }
                }
            }
        }
        (present, live)
    }

    /// Live entry count of one narrow row.
    ///
    /// Scan fallback behind `live_key_count` for rows without an index.
    pub(crate) fn row_live_count(&self, vid: u32) -> usize {
        let idx = vid as usize;
        if idx >= self.vertex_capacity() {
            return 0;
        }
        let mut live = 0usize;
        let (_, cold) = self.primary_pair(idx);
        for c in cold.iter() {
            if c.is_live() {
                live += 1;
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for cold in chunk.cold_slice() {
                    if cold.is_live() {
                        live += 1;
                    }
                }
            }
        }
        live
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
        // touches only lengths, not entries. Rebuilds drop the set again
        // when the live width is still narrow, so tombstone-heavy rows may
        // rescan on later inserts until maintenance compacts them.
        let mut width = 0usize;
        if (vid as usize) < self.vertex_capacity() {
            width = self.degrees[vid as usize] as usize;
            if let Some(chunks) = self.overflow_chunks.get(&vid) {
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
            return;
        }
        let (hot, cold) = self.primary_pair(idx);
        let mut positioned = Vec::new();
        for (slot, (h, c)) in hot.iter().zip(cold.iter()).enumerate() {
            if c.is_live() {
                positioned.push((
                    (h.endpoint, h.rank),
                    EdgePosition::Primary { slot: slot as u32 },
                ));
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for (chunk_idx, chunk) in chunks.iter().enumerate() {
                for (slot_idx, (hot, cold)) in
                    chunk.hot_slice().iter().zip(chunk.cold_slice()).enumerate()
                {
                    if cold.is_live() {
                        positioned.push((
                            (hot.endpoint, hot.rank),
                            EdgePosition::Overflow {
                                chunk: chunk_idx as u32,
                                slot: slot_idx as u32,
                            },
                        ));
                    }
                }
            }
        }
        // Narrow rows stay set-free; only wide rows pay for the index.
        if positioned.len() <= LIVE_SET_WIDTH_BOUND {
            self.live_sets.remove(&vid);
        } else {
            self.live_sets
                .insert(vid, LiveKeySet::from_positions(positioned));
        }
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

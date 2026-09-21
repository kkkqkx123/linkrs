use std::collections::HashMap;

use super::super::csr_shared::SegmentedTable;
use super::super::EdgePosition;

#[derive(Debug, Clone, Default)]
pub(crate) struct PureLiveKeySet {
    positions: HashMap<u32, EdgePosition>,
}

impl PureLiveKeySet {
    pub(crate) fn from_positions(positions: Vec<(u32, EdgePosition)>) -> Self {
        Self {
            positions: positions.into_iter().collect(),
        }
    }

    #[inline]
    pub(crate) fn contains(&self, endpoint: &u32) -> bool {
        self.positions.contains_key(endpoint)
    }

    #[inline]
    pub(crate) fn position(&self, endpoint: &u32) -> Option<EdgePosition> {
        self.positions.get(endpoint).copied()
    }

    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.positions.len()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    #[inline]
    pub(crate) fn insert(&mut self, endpoint: u32, position: EdgePosition) {
        self.positions.insert(endpoint, position);
    }

    #[inline]
    pub(crate) fn remove(&mut self, endpoint: &u32) {
        self.positions.remove(endpoint);
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.positions.len() * (std::mem::size_of::<(u32, EdgePosition)>() + 8)
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PureLiveSetStorage {
    table: SegmentedTable<PureLiveKeySet>,
    live_rows: usize,
}

impl PureLiveSetStorage {
    pub(crate) fn new() -> Self {
        Self {
            table: SegmentedTable::new(),
            live_rows: 0,
        }
    }

    pub(crate) fn ensure_capacity(&mut self, vertex_capacity: usize) {
        self.table.ensure_capacity(vertex_capacity);
    }

    #[inline]
    pub(crate) fn get(&self, vid: u32) -> Option<&PureLiveKeySet> {
        self.table.get(vid)
    }

    pub(crate) fn insert(&mut self, vid: u32, set: PureLiveKeySet) {
        let slot = self.table.slot_mut(vid);
        if slot.is_none() {
            self.live_rows += 1;
        }
        *slot = Some(set);
    }

    pub(crate) fn remove(&mut self, vid: u32) {
        if self.table.take(vid).is_some() {
            self.live_rows = self.live_rows.saturating_sub(1);
        }
    }

    pub(crate) fn insert_key(&mut self, vid: u32, endpoint: u32, position: EdgePosition) {
        let slot = self.table.slot_mut(vid);
        match slot {
            Some(set) => set.insert(endpoint, position),
            slot @ None => {
                *slot = Some(PureLiveKeySet::from_positions(vec![(endpoint, position)]));
                self.live_rows += 1;
            }
        }
    }

    pub(crate) fn remove_key(&mut self, vid: u32, endpoint: &u32) {
        let emptied = match self.table.get_mut(vid) {
            Some(set) => {
                set.remove(endpoint);
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
            + self.live_rows * (std::mem::size_of::<u32>() + std::mem::size_of::<PureLiveKeySet>())
    }
}

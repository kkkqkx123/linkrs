use std::collections::HashSet;

use super::super::Timestamp;
use super::MutableCsr;

impl MutableCsr {
    pub(crate) fn rebuild_live_sets(&mut self) {
        let capacity = self.vertex_capacity() as u32;
        self.live_sets.clear();
        for vid in 0..capacity {
            self.rebuild_live_set_for_vertex(vid);
        }
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
        let mut set = HashSet::new();
        for i in 0..degree {
            if let Some(nbr) = self.nbr_list.get(offset + i) {
                if nbr.delete_ts == Timestamp::MAX {
                    set.insert((nbr.endpoint, nbr.rank));
                }
            }
        }
        if let Some(chunks) = self.overflow_chunks.get(&vid) {
            for chunk in chunks {
                for nbr in chunk {
                    if nbr.delete_ts == Timestamp::MAX {
                        set.insert((nbr.endpoint, nbr.rank));
                    }
                }
            }
        }
        if set.is_empty() {
            self.live_sets.remove(&vid);
        } else {
            self.live_sets.insert(vid, set);
        }
    }
}

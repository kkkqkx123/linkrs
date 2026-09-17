use std::collections::HashMap;

use super::super::Nbr;

/// Single benchmarked per-vertex overflow bound. Past this many chunks a row
/// holding dead entries is repacked on the write path; a row holding only
/// live entries waits for a region or full compaction instead of repeatedly
/// repacking live data. No alternative threshold set is retained.
pub(crate) const OVERFLOW_REPACK_CHUNKS_PER_VERTEX: usize = 8;

/// Per-vertex overflow storage keyed by vertex id for constant-time lookup.
///
/// Each vertex owns an independent chunk list; vertex insert and removal never
/// move other vertices, so hot vertices stay bounded by their own block count.
#[derive(Debug, Clone, Default)]
pub struct OverflowStorage {
    pub(crate) map: HashMap<u32, Vec<Vec<Nbr>>>,
}

impl OverflowStorage {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    #[inline]
    pub fn get(&self, vid: &u32) -> Option<&Vec<Vec<Nbr>>> {
        self.map.get(vid)
    }

    #[inline]
    pub fn get_mut(&mut self, vid: &u32) -> Option<&mut Vec<Vec<Nbr>>> {
        self.map.get_mut(vid)
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<Vec<Nbr>> {
        self.map.entry(vid).or_default()
    }

    #[inline]
    pub fn insert(&mut self, vid: u32, chunks: Vec<Vec<Nbr>>) {
        self.map.insert(vid, chunks);
    }

    #[inline]
    pub fn contains_key(&self, vid: &u32) -> bool {
        self.map.contains_key(vid)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.map.clear();
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Vec<Vec<Nbr>>)> {
        self.map.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&u32, &mut Vec<Vec<Nbr>>)> {
        self.map.iter_mut()
    }

    /// Remove entry for `vid` and return its chunks if present.
    #[inline]
    pub fn remove(&mut self, vid: &u32) -> Option<Vec<Vec<Nbr>>> {
        self.map.remove(vid)
    }

    /// Total number of Nbr entries across all overflow chunks.
    pub fn total_nbr_count(&self) -> usize {
        self.map
            .values()
            .map(|chunks| chunks.iter().map(Vec::len).sum::<usize>())
            .sum()
    }

    /// Estimate of wasted capacity inside overflow chunks (capacity - len).
    pub fn wasted_capacity(&self) -> usize {
        self.map
            .values()
            .map(|chunks| {
                chunks
                    .iter()
                    .map(|c| c.capacity().saturating_sub(c.len()))
                    .sum::<usize>()
            })
            .sum()
    }

    /// Check whether merging overflow back into primary would be beneficial.
    pub fn should_merge(&self, fragmentation_threshold: f32) -> bool {
        let total: usize = self.total_nbr_count();
        if total == 0 {
            return false;
        }
        let wasted = self.wasted_capacity();
        let ratio = wasted as f32 / (total + wasted) as f32;
        ratio > fragmentation_threshold
    }
}

use std::collections::HashMap;

use super::super::Nbr;

pub(crate) const SEQUENTIAL_RUN_THRESHOLD: usize = 16;
pub(crate) const MAX_OVERFLOW_CHUNKS_PER_VERTEX: usize = 32;

/// Sequential overflow run: contiguous vertex range with uniform chunk count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequentialRun {
    pub start_vid: u32,
    pub vertex_count: u32,
    pub chunk_count: usize,
}

/// Optimized overflow chunk index for sequential vertex ranges.
#[derive(Debug, Clone, Default)]
pub struct OverflowIndex {
    sequential_runs: Vec<SequentialRun>,
}

impl OverflowIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sequential_runs(&self) -> &[SequentialRun] {
        &self.sequential_runs
    }

    pub fn len(&self) -> usize {
        self.sequential_runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sequential_runs.is_empty()
    }

    pub fn clear(&mut self) {
        self.sequential_runs.clear();
    }

    /// Check if a vertex belongs to any sequential run.
    pub fn is_sequential(&self, vid: u32) -> bool {
        self.sequential_runs
            .iter()
            .any(|run| vid >= run.start_vid && vid < run.start_vid + run.vertex_count)
    }

    /// Find the sequential run containing `vid`, if any.
    pub fn find_run(&self, vid: u32) -> Option<&SequentialRun> {
        self.sequential_runs
            .iter()
            .find(|run| vid >= run.start_vid && vid < run.start_vid + run.vertex_count)
    }

    /// Build index from overflow_chunks map.
    pub fn rebuild_from_chunks(chunks: &HashMap<u32, Vec<Vec<Nbr>>>) -> Self {
        if chunks.is_empty() {
            return Self::default();
        }
        let mut vertices: Vec<(u32, usize)> =
            chunks.iter().map(|(vid, v)| (*vid, v.len())).collect();
        vertices.sort_by_key(|(vid, _)| *vid);

        let mut runs = Vec::new();
        let mut i = 0usize;
        while i < vertices.len() {
            let (vid, chunk_count) = vertices[i];
            let mut run_length = 1usize;
            while i + run_length < vertices.len() {
                let (next_vid, next_count) = vertices[i + run_length];
                if next_vid != vid + run_length as u32 || next_count != chunk_count {
                    break;
                }
                run_length += 1;
            }
            if run_length >= SEQUENTIAL_RUN_THRESHOLD {
                runs.push(SequentialRun {
                    start_vid: vid,
                    vertex_count: run_length as u32,
                    chunk_count,
                });
                i += run_length;
            } else {
                i += 1;
            }
        }
        Self {
            sequential_runs: runs,
        }
    }

    /// Build index from overflow storage (sorted-vector backend).
    pub fn rebuild_from_storage(storage: &OverflowStorage) -> Self {
        if storage.is_empty() {
            return Self::default();
        }
        let mut vertices: Vec<(u32, usize)> =
            storage.iter().map(|(vid, v)| (*vid, v.len())).collect();
        vertices.sort_by_key(|(vid, _)| *vid);

        let mut runs = Vec::new();
        let mut i = 0usize;
        while i < vertices.len() {
            let (vid, chunk_count) = vertices[i];
            let mut run_length = 1usize;
            while i + run_length < vertices.len() {
                let (next_vid, next_count) = vertices[i + run_length];
                if next_vid != vid + run_length as u32 || next_count != chunk_count {
                    break;
                }
                run_length += 1;
            }
            if run_length >= SEQUENTIAL_RUN_THRESHOLD {
                runs.push(SequentialRun {
                    start_vid: vid,
                    vertex_count: run_length as u32,
                    chunk_count,
                });
                i += run_length;
            } else {
                i += 1;
            }
        }
        Self {
            sequential_runs: runs,
        }
    }

    /// Memory savings estimate for overflow metadata (in bytes).
    pub fn metadata_bytes_saved(&self, total_overflow_vertices: usize) -> usize {
        if self.sequential_runs.is_empty() {
            return 0;
        }
        let sequential_vertices: usize = self
            .sequential_runs
            .iter()
            .map(|r| r.vertex_count as usize)
            .sum();
        let sparse_vertices = total_overflow_vertices.saturating_sub(sequential_vertices);
        let before = total_overflow_vertices * (4 + 24);
        let after = sparse_vertices * (4 + 24) + self.sequential_runs.len() * 12;
        before.saturating_sub(after)
    }
}

/// Statistics for overflow index.
#[derive(Debug, Clone, Copy)]
pub struct OverflowIndexStats {
    pub total_overflow_vertices: usize,
    pub sequential_runs: usize,
    pub sequential_vertices: usize,
    pub sparse_vertices: usize,
    pub metadata_bytes_saved: usize,
}

/// Sorted-vector based overflow storage replacing `HashMap<u32, Vec<Vec<Nbr>>>`.
#[derive(Debug, Clone, Default)]
pub struct OverflowStorage {
    pub(crate) entries: Vec<(u32, Vec<Vec<Nbr>>)>,
}

impl OverflowStorage {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    #[inline]
    pub(crate) fn find_index(&self, vid: &u32) -> Result<usize, usize> {
        self.entries.binary_search_by_key(vid, |(k, _)| *k)
    }

    #[inline]
    pub fn get(&self, vid: &u32) -> Option<&Vec<Vec<Nbr>>> {
        match self.find_index(vid) {
            Ok(idx) => Some(&self.entries[idx].1),
            Err(_) => None,
        }
    }

    #[inline]
    pub fn get_mut(&mut self, vid: &u32) -> Option<&mut Vec<Vec<Nbr>>> {
        match self.find_index(vid) {
            Ok(idx) => Some(&mut self.entries[idx].1),
            Err(_) => None,
        }
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent while keeping the vector sorted.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<Vec<Nbr>> {
        match self.find_index(&vid) {
            Ok(idx) => &mut self.entries[idx].1,
            Err(idx) => {
                self.entries.insert(idx, (vid, Vec::new()));
                &mut self.entries[idx].1
            }
        }
    }

    #[inline]
    pub fn insert(&mut self, vid: u32, chunks: Vec<Vec<Nbr>>) {
        match self.find_index(&vid) {
            Ok(idx) => self.entries[idx].1 = chunks,
            Err(idx) => self.entries.insert(idx, (vid, chunks)),
        }
    }

    #[inline]
    pub fn contains_key(&self, vid: &u32) -> bool {
        self.find_index(vid).is_ok()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, (u32, Vec<Vec<Nbr>>)> {
        self.entries.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, (u32, Vec<Vec<Nbr>>)> {
        self.entries.iter_mut()
    }

    /// Remove entry for `vid` and return its chunks if present.
    #[inline]
    pub fn remove(&mut self, vid: &u32) -> Option<Vec<Vec<Nbr>>> {
        match self.find_index(vid) {
            Ok(idx) => Some(self.entries.remove(idx).1),
            Err(_) => None,
        }
    }

    /// Total number of Nbr entries across all overflow chunks.
    pub fn total_nbr_count(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, chunks)| chunks.iter().map(Vec::len).sum::<usize>())
            .sum()
    }

    /// Estimate of wasted capacity inside overflow chunks (capacity - len).
    pub fn wasted_capacity(&self) -> usize {
        self.entries
            .iter()
            .map(|(_, chunks)| {
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

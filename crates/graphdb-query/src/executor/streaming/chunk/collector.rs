//! Terminal result collector: thread-local chunk accumulation.
//!
//! Ladybug `ResultCollector` analogue at the row level: each pipeline owns a
//! `LocalChunkCollector`, moves visible rows out of chunks exactly once
//! (`DataChunk::expand_visible_rows`, selection + multiplicity aware), and
//! merges collectors by moving whole blocks instead of per-Value clones.
//!
//! Storage is block-partitioned ([`COLLECTOR_BLOCK_ROWS`] rows per block) so
//! large results avoid the single-`Vec` reallocation peak and can be consumed
//! through the [`LocalChunkCollector::iter_rows`] cursor without flattening.
//! Disk spill hooks (`set_spill_threshold_rows`, `spill_threshold_exceeded`,
//! `is_spilled`) are reserved for the spill-through-collect follow-up; this
//! stage keeps everything in memory (`is_spilled` is always false).

use super::core::DataChunk;
use graphdb_core::Value;

/// Rows per collector block. Bounds the reallocation peak of a single push.
pub const COLLECTOR_BLOCK_ROWS: usize = 4096;

/// Thread-local terminal accumulator for query results.
#[derive(Debug, Default)]
pub struct LocalChunkCollector {
    blocks: Vec<Vec<Vec<Value>>>,
    col_names: Vec<String>,
    len: usize,
    total_logical: u128,
    spill_threshold_rows: Option<u64>,
}

impl LocalChunkCollector {
    pub fn new(col_names: Vec<String>) -> Self {
        Self {
            blocks: Vec::new(),
            col_names,
            len: 0,
            total_logical: 0,
            spill_threshold_rows: None,
        }
    }

    /// Move all visible (expanded) rows of `chunk` into the collector,
    /// splitting them across fixed-size blocks.
    pub fn push_chunk(&mut self, chunk: &mut DataChunk) {
        if self.col_names.is_empty() {
            self.col_names = chunk.col_names();
        }
        self.total_logical += u128::from(chunk.logical_len());
        let mut rows = chunk.expand_visible_rows();
        if rows.is_empty() {
            return;
        }
        // Drain from the front without shifting: reverse once, then pop.
        rows.reverse();
        while !rows.is_empty() {
            let tail_full = self
                .blocks
                .last()
                .is_none_or(|b| b.len() >= COLLECTOR_BLOCK_ROWS);
            if tail_full {
                self.blocks
                    .push(Vec::with_capacity(rows.len().min(COLLECTOR_BLOCK_ROWS)));
            }
            let tail = self.blocks.last_mut().expect("tail block must exist");
            while tail.len() < COLLECTOR_BLOCK_ROWS {
                if let Some(row) = rows.pop() {
                    tail.push(row);
                    self.len += 1;
                } else {
                    break;
                }
            }
        }
    }

    /// Merge another collector by moving whole blocks (no per-Value clone).
    pub fn merge(&mut self, mut other: LocalChunkCollector) {
        if self.col_names.is_empty() {
            self.col_names = std::mem::take(&mut other.col_names);
        }
        self.total_logical += other.total_logical;
        self.len += other.len;
        if self.blocks.is_empty() {
            self.blocks = other.blocks;
            return;
        }
        for mut block in other.blocks {
            if block.is_empty() {
                continue;
            }
            let tail_has_room = self
                .blocks
                .last()
                .is_some_and(|b| b.len() < COLLECTOR_BLOCK_ROWS);
            if !tail_has_room {
                self.blocks.push(std::mem::take(&mut block));
                continue;
            }
            // Fill the tail first, then move the remainder as a new block.
            let tail = self.blocks.last_mut().expect("tail block must exist");
            while tail.len() < COLLECTOR_BLOCK_ROWS && !block.is_empty() {
                // `block` was filled front-to-back; pop from the back into a
                // staging vec would reverse order, so drain from the front in
                // bulk instead.
                let room = COLLECTOR_BLOCK_ROWS - tail.len();
                let take = room.min(block.len());
                tail.extend(block.drain(..take));
            }
            if !block.is_empty() {
                self.blocks.push(block);
            }
        }
    }

    /// Expanded row count observed (selection + multiplicity applied).
    pub fn total_logical_rows(&self) -> u128 {
        self.total_logical
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    pub fn into_rows(self) -> (Vec<Vec<Value>>, Vec<String>) {
        let mut out = Vec::with_capacity(self.len);
        for block in self.blocks {
            out.extend(block);
        }
        (out, self.col_names)
    }

    /// Block-partitioned rows (existing callers via `into_rows` are unaffected).
    pub fn into_blocks(self) -> (Vec<Vec<Vec<Value>>>, Vec<String>) {
        (self.blocks, self.col_names)
    }

    /// Ordered row cursor over blocks, for streaming/paged consumers.
    pub fn iter_rows(&self) -> impl Iterator<Item = &Vec<Value>> {
        self.blocks.iter().flatten()
    }

    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }

    /// Disk-spill reservation: always false in this stage (in-memory only).
    pub fn is_spilled(&self) -> bool {
        false
    }

    /// Configure the logical-row threshold that will trigger spill-through
    /// collect once the `SpillManager` wiring lands.
    pub fn set_spill_threshold_rows(&mut self, threshold: u64) {
        self.spill_threshold_rows = Some(threshold);
    }

    /// Whether the configured spill threshold has been exceeded.
    pub fn spill_threshold_exceeded(&self) -> bool {
        match self.spill_threshold_rows {
            Some(t) => u128::from(t) < self.total_logical,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::slot::SlotLayout;
    use std::sync::Arc;

    fn chunk_with(rows: Vec<Vec<Value>>, selection: Option<Vec<usize>>, mult: u64) -> DataChunk {
        let layout = Arc::new(SlotLayout::from_names(&["a".to_string()]));
        let mut c = DataChunk::new_with_layout(rows, layout);
        if let Some(sel) = selection {
            c = c.with_selection(sel);
        }
        c.with_multiplicity(mult)
    }

    #[test]
    fn expands_selection_and_multiplicity_once() {
        let mut c = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            Some(vec![0, 2]),
            2,
        );
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.push_chunk(&mut c);
        assert_eq!(collector.total_logical_rows(), 4);
        assert_eq!(collector.len(), 4);
        assert!(c.rows.is_empty());
    }

    #[test]
    fn merge_sums_counts() {
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut c1 = chunk_with(vec![vec![Value::Int(1)]], None, 1);
        let mut c2 = chunk_with(vec![vec![Value::Int(2)]], None, 3);
        a.push_chunk(&mut c1);
        b.push_chunk(&mut c2);
        a.merge(b);
        assert_eq!(a.total_logical_rows(), 4);
        assert_eq!(a.len(), 4);
    }

    #[test]
    fn splits_large_push_into_blocks() {
        let rows: Vec<Vec<Value>> = (0..5000).map(|i| vec![Value::Int(i as i32)]).collect();
        let mut c = chunk_with(rows, None, 1);
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        collector.push_chunk(&mut c);
        assert_eq!(collector.len(), 5000);
        assert_eq!(collector.num_blocks(), 2);
        let (flat, _) = LocalChunkCollector {
            blocks: collector.into_blocks().0,
            col_names: vec!["a".to_string()],
            len: 5000,
            total_logical: 5000,
            spill_threshold_rows: None,
        }
        .into_rows();
        assert_eq!(flat.len(), 5000);
        assert_eq!(flat[0], vec![Value::Int(0)]);
        assert_eq!(flat[4999], vec![Value::Int(4999)]);
    }

    #[test]
    fn merge_moves_blocks_and_iter_rows_preserves_order() {
        let mut a = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut b = LocalChunkCollector::new(vec!["a".to_string()]);
        let mut c1 = chunk_with(vec![vec![Value::Int(1)], vec![Value::Int(2)]], None, 1);
        let mut c2 = chunk_with(vec![vec![Value::Int(3)]], None, 1);
        a.push_chunk(&mut c1);
        b.push_chunk(&mut c2);
        a.merge(b);
        let ordered: Vec<i32> = a
            .iter_rows()
            .map(|r| match &r[0] {
                Value::Int(v) => *v,
                _ => -1,
            })
            .collect();
        assert_eq!(ordered, vec![1, 2, 3]);
        let (flat, _) = a.into_rows();
        assert_eq!(flat.len(), 3);
    }

    #[test]
    fn spill_threshold_hook_defaults_off() {
        let mut collector = LocalChunkCollector::new(vec!["a".to_string()]);
        assert!(!collector.is_spilled());
        assert!(!collector.spill_threshold_exceeded());
        collector.set_spill_threshold_rows(2);
        let mut c = chunk_with(
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
            None,
            1,
        );
        collector.push_chunk(&mut c);
        assert!(collector.spill_threshold_exceeded());
        assert!(!collector.is_spilled());
    }
}

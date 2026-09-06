//! Terminal result collector: thread-local chunk accumulation.
//!
//! Ladybug `ResultCollector` analogue at the row level: each pipeline owns a
//! `LocalChunkCollector`, moves visible rows out of chunks exactly once
//! (`DataChunk::expand_visible_rows`, selection + multiplicity aware), and
//! merges collectors with a single `extend` instead of per-Value clones.

use super::core::DataChunk;
use graphdb_core::Value;

/// Thread-local terminal accumulator for query results.
#[derive(Debug, Default)]
pub struct LocalChunkCollector {
    rows: Vec<Vec<Value>>,
    col_names: Vec<String>,
    total_logical: u128,
}

impl LocalChunkCollector {
    pub fn new(col_names: Vec<String>) -> Self {
        Self {
            rows: Vec::new(),
            col_names,
            total_logical: 0,
        }
    }

    /// Move all visible (expanded) rows of `chunk` into the collector.
    pub fn push_chunk(&mut self, chunk: &mut DataChunk) {
        if self.col_names.is_empty() {
            self.col_names = chunk.col_names();
        }
        self.total_logical += u128::from(chunk.logical_len());
        self.rows.extend(chunk.expand_visible_rows());
    }

    /// Merge another collector with a single row-vector `extend`.
    pub fn merge(&mut self, mut other: LocalChunkCollector) {
        if self.col_names.is_empty() {
            self.col_names = std::mem::take(&mut other.col_names);
        }
        self.total_logical += other.total_logical;
        self.rows.extend(other.rows);
    }

    /// Expanded row count observed (selection + multiplicity applied).
    pub fn total_logical_rows(&self) -> u128 {
        self.total_logical
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    pub fn into_rows(self) -> (Vec<Vec<Value>>, Vec<String>) {
        (self.rows, self.col_names)
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
}

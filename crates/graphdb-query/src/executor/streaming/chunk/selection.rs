//! Selection vectors, index-based take, and slice operations
//!
//! # Selection propagation contract
//!
//! Operators fall into two categories:
//! - **Transparent**: `Filter`, `Project` (via `evaluate_expression_visible`),
//!   `Limit`/`Offset`, and stateless unary operators. They consume the
//!   selection vector without moving rows, so chunks stay compact.
//!   [`DataChunk::selection`] hands the visible indices to such consumers.
//! - **Opaque**: blocking operators (aggregate/sort) and join builds (hash
//!   join). They own the rows and must call
//!   [`DataChunk::materialize_selection_by`] — the selection degenerates into
//!   a compact row batch at the boundary.
//!
//! # Multiplicity propagation contract
//!
//! `multiplicity` is a symbolic per-chunk row factor carried alongside the
//! selection vector:
//! - Row-preserving rebuilds (`Project`/`Assign`/`Remove`/`AppendVertices`/
//!   `Sample`/`Unwind`) must carry it with `with_multiplicity(source)`,
//!   because every output row still occurs `multiplicity` times.
//! - Row-collapsing operators (`Dedup`/`Distinct`/aggregates/sets) start a new
//!   grouping, so `multiplicity` resets to 1 (the constructor default).
//! - Opaque order-preserving operators (`Sort`/`TopN`/`Window`/join probe)
//!   merge chunks whose uniform factors cannot be represented after the
//!   merge; they must enter through [`DataChunk::normalize_for_opaque`],
//!   which expands `multiplicity` and materializes the selection together.
//!   Calling [`DataChunk::materialize_selection_by`] alone is not enough:
//!   it leaves `multiplicity > 1` behind and downstream row counts shrink.

use super::pool::RowBufferPool;
use super::typed::{gather_typed_column, repeat_typed_column};
use crate::executor::streaming::chunk::core::DataChunk;

impl DataChunk {
    // ── Multiplicity (symbolic row duplication) ──

    /// Symbolic multiplicity: how many times each visible row occurs.
    pub fn multiplicity(&self) -> u64 {
        self.multiplicity
    }

    pub fn set_multiplicity(&mut self, multiplicity: u64) {
        debug_assert!(multiplicity >= 1, "multiplicity must be >= 1");
        self.multiplicity = multiplicity.max(1);
    }

    pub fn with_multiplicity(mut self, multiplicity: u64) -> Self {
        self.set_multiplicity(multiplicity);
        self
    }

    /// Expanded (flat) visible row count: `visible_count * multiplicity`.
    /// Saturates instead of overflowing so metrics paths stay total.
    pub fn logical_len(&self) -> u64 {
        (self.visible_count() as u64).saturating_mul(self.multiplicity)
    }

    /// Borrow visible rows in upstream order (respects `selection`).
    pub fn visible_rows(&self) -> impl Iterator<Item = &Vec<graphdb_core::Value>> {
        match &self.selection {
            Some(indices) => VisibleRows::Selected {
                rows: &self.rows,
                indices,
                pos: 0,
            },
            None => VisibleRows::All {
                rows: &self.rows,
                pos: 0,
            },
        }
    }

    /// Move visible rows out, repeating each row `multiplicity` times.
    ///
    /// This is the single terminal move: rows are moved out with `mem::take`
    /// with no per-Value clone on the move itself (`multiplicity == 1` returns the moved rows
    /// directly; larger factors repeat them). After the call the chunk is
    /// empty with `selection=None` and derived column caches cleared.
    ///
    /// Opaque order-preserving operators assume `multiplicity == 1`; a future
    /// multiplicity producer must expand at those boundaries with this call
    /// first (see the module-level multiplicity contract).
    pub fn expand_visible_rows(&mut self) -> Vec<Vec<graphdb_core::Value>> {
        let multiplicity = self.multiplicity;
        // Capture the visible indices before the selection is taken so the
        // typed layout can be re-expanded in lockstep with the rows.
        let visible_indices: Vec<usize> = match &self.selection {
            Some(indices) => indices.clone(),
            None => (0..self.rows.len()).collect(),
        };
        let out = match self.selection.take() {
            Some(indices) => {
                let mut selected = Vec::with_capacity(indices.len());
                for &i in &indices {
                    selected.push(std::mem::take(&mut self.rows[i]));
                }
                self.rows.clear();
                selected
            }
            None => std::mem::take(&mut self.rows),
        };
        self.columns = None;
        // Keep the typed layout across the expansion (same ownership rule as
        // `materialize_selection_inner`): gather the visible rows, then repeat
        // each per `multiplicity`, so downstream operators still see a typed
        // columnar chunk.
        self.typed_columns = self.typed_columns.take().map(|cols| {
            cols.iter()
                .map(|col| {
                    let gathered = gather_typed_column(col, &visible_indices);
                    if multiplicity <= 1 {
                        gathered
                    } else {
                        repeat_typed_column(&gathered, multiplicity as usize)
                    }
                })
                .collect()
        });
        self.multiplicity = 1;
        debug_assert!(
            self.selection.is_none() && self.multiplicity == 1,
            "expand_visible_rows must leave a flat chunk: \
             selection consumed, multiplicity reset to 1"
        );
        if multiplicity <= 1 || out.is_empty() {
            return out;
        }
        let total = (out.len() as u64).saturating_mul(multiplicity) as usize;
        if let Some(stats) = &self.columnar_stats {
            stats.record_multiplicity_expanded(total as u64);
        }
        expand_rows_reusing_buffers(out, multiplicity, total)
    }

    // ── Selection vectors ──

    pub fn with_selection(mut self, indices: Vec<usize>) -> Self {
        debug_assert!(indices.is_sorted() && indices.windows(2).all(|w| w[0] < w[1]));
        debug_assert!(indices.last().is_none_or(|&i| i < self.rows.len()));
        self.selection = Some(indices);
        self
    }

    pub fn selection(&self) -> Option<&[usize]> {
        self.selection.as_deref()
    }

    pub fn visible_count(&self) -> usize {
        self.selection
            .as_ref()
            .map(Vec::len)
            .unwrap_or(self.rows.len())
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        match &self.selection {
            Some(indices) => indices.clone(),
            None => (0..self.rows.len()).collect(),
        }
    }

    pub fn is_visible(&self, idx: usize) -> bool {
        match &self.selection {
            None => idx < self.rows.len(),
            Some(indices) => indices.binary_search(&idx).is_ok(),
        }
    }

    pub fn take_selection(&mut self) -> Option<Vec<usize>> {
        self.selection.take()
    }

    /// Expand a symbolic `multiplicity > 1` into physical rows in place.
    ///
    /// Opaque operators (sort/window/join/aggregate) cannot carry the factor
    /// symbolically, so they must call this (directly or via
    /// [`DataChunk::normalize_for_opaque`]) before consuming rows.
    /// Returns `true` when an expansion happened. Visible selection is
    /// preserved: only visible rows are replicated, hidden rows are dropped.
    /// Derived column caches are cleared, matching `expand_visible_rows`.
    pub fn expand_multiplicity_in_place(&mut self) -> bool {
        if self.multiplicity <= 1 {
            return false;
        }
        let multiplicity = self.multiplicity;
        // Capture the visible indices before the selection is taken so the
        // typed layout can be re-expanded in lockstep with the rows.
        let visible_indices: Vec<usize> = match &self.selection {
            Some(indices) => indices.clone(),
            None => (0..self.rows.len()).collect(),
        };
        // Take visible rows once (same ownership discipline as
        // `expand_visible_rows`); hidden rows are dropped.
        let taken: Vec<Vec<graphdb_core::Value>> = match self.selection.take() {
            Some(indices) => {
                let mut selected = Vec::with_capacity(indices.len());
                for &i in &indices {
                    selected.push(std::mem::take(&mut self.rows[i]));
                }
                self.rows.clear();
                selected
            }
            None => std::mem::take(&mut self.rows),
        };
        self.columns = None;
        // Keep the typed layout across the expansion (consistent with
        // `materialize_selection_inner`): gather the visible rows, then repeat
        // each per `multiplicity`.
        self.typed_columns = self.typed_columns.take().map(|cols| {
            cols.iter()
                .map(|col| {
                    let gathered = gather_typed_column(col, &visible_indices);
                    repeat_typed_column(&gathered, multiplicity as usize)
                })
                .collect()
        });
        self.multiplicity = 1;
        debug_assert!(
            self.selection.is_none() && self.multiplicity == 1,
            "expand_multiplicity_in_place must leave a flat chunk: \
             selection consumed, multiplicity reset to 1"
        );
        if taken.is_empty() {
            return true;
        }
        let total = (taken.len() as u64).saturating_mul(multiplicity) as usize;
        self.rows = expand_rows_reusing_buffers(taken, multiplicity, total);
        if let Some(stats) = &self.columnar_stats {
            stats.record_multiplicity_expanded(total as u64);
        }
        true
    }

    /// Unified opaque-operator boundary: expand `multiplicity`, then
    /// materialize any attached selection. Returns `true` when either step
    /// changed the chunk. `op` must be one of `SELECTION_BOUNDARY_OPS`.
    ///
    /// This is the single entry point opaque operators should use instead of
    /// calling [`DataChunk::materialize_selection_by`] directly.
    pub fn normalize_for_opaque(&mut self, op: &'static str) -> bool {
        let expanded = self.expand_multiplicity_in_place();
        let materialized = self.materialize_selection_by(op);
        expanded || materialized
    }

    /// Contract: materialize any attached selection (opaque-operator
    /// boundary). Returns `true` when a selection was actually materialized.
    ///
    /// Prefer [`DataChunk::normalize_for_opaque`]: this method alone does not
    /// expand `multiplicity`, so opaque operators calling it directly must
    /// guarantee `multiplicity == 1` (debug-asserted by callers via the
    /// normalize path).
    ///
    /// This is the single boundary-materialization entry point; `op` must be
    /// one of `SELECTION_BOUNDARY_OPS` so per-operator counters stay exact
    /// (ad-hoc names fall back to the unattributed counter).
    pub fn materialize_selection_by(&mut self, op: &'static str) -> bool {
        let did = self.materialize_selection_inner();
        if did {
            if let Some(stats) = &self.columnar_stats {
                stats.record_selection_materialized_by(op);
            }
        }
        did
    }

    fn materialize_selection_inner(&mut self) -> bool {
        let Some(indices) = self.selection.take() else {
            return false;
        };
        let mut selected = Vec::with_capacity(indices.len());
        for &i in &indices {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        self.rows = selected;
        self.columns = self.columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| indices.iter().map(|&i| col[i].clone()).collect())
                .collect()
        });
        self.typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, &indices))
                .collect()
        });
        true
    }

    // ── Index-based take & slice ──

    pub fn take_indices(&mut self, indices: &[usize]) -> Self {
        let layout = std::sync::Arc::clone(&self.layout);
        let schema = std::sync::Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(indices.len());
        for &i in indices {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        let typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, indices))
                .collect()
        });
        Self {
            rows: selected,
            columns: None,
            typed_columns,
            selection: None,
            multiplicity: self.multiplicity,
            schema,
            layout,
            memory_reservation: self.memory_reservation.take(),
            columnar_stats: self.columnar_stats.clone(),
        }
    }

    pub fn slice(&mut self, start: usize, end: usize) -> Self {
        assert!(end <= self.rows.len(), "slice end out of bounds");
        let layout = std::sync::Arc::clone(&self.layout);
        let schema = std::sync::Arc::clone(&self.schema);
        let mut selected = Vec::with_capacity(end - start);
        for i in start..end {
            selected.push(std::mem::take(&mut self.rows[i]));
        }
        let indices: Vec<usize> = (start..end).collect();
        let typed_columns = self.typed_columns.as_ref().map(|cols| {
            cols.iter()
                .map(|col| gather_typed_column(col, &indices))
                .collect()
        });
        Self {
            rows: selected,
            columns: None,
            typed_columns,
            selection: None,
            multiplicity: self.multiplicity,
            schema,
            layout,
            memory_reservation: self.memory_reservation.take(),
            columnar_stats: self.columnar_stats.clone(),
        }
    }
}

/// Physically repeat `source` rows `multiplicity` times, reusing pooled
/// row buffers for the copies.
///
/// The first repetition moves the source buffers (no per-Value clone); each
/// further repetition clones into a recycled buffer from
/// [`RowBufferPool`] (falling back to a fresh allocation when the pool is
/// dry). Repetition order matches the old `extend`-loop: all visible rows,
/// then the same sequence again. Callers pass the already-computed expanded
/// `total` for capacity planning; growth past `1 << 22` rows reserves in
/// bounded steps, mirroring the previous behavior.
fn expand_rows_reusing_buffers(
    source: Vec<Vec<graphdb_core::Value>>,
    multiplicity: u64,
    total: usize,
) -> Vec<Vec<graphdb_core::Value>> {
    debug_assert!(multiplicity > 1, "pool path is only for real expansion");
    debug_assert!(!source.is_empty(), "empty source needs no expansion");
    let round = source.len();
    let mut expanded = Vec::with_capacity(total.min(1 << 22));
    expanded.extend(source);
    // One pool lock per expansion: grab every buffer the clone rounds need.
    let mut recycled = RowBufferPool::acquire_rows(total.saturating_sub(round));
    for _ in 1..multiplicity {
        for i in 0..round {
            let mut buf = recycled.pop().unwrap_or_default();
            buf.extend(expanded[i].iter().cloned());
            expanded.push(buf);
        }
        if expanded.len() >= (1 << 22) {
            expanded.reserve(total.saturating_sub(expanded.len()).min(1 << 22));
        }
    }
    expanded
}

enum VisibleRows<'a> {
    All {
        rows: &'a [Vec<graphdb_core::Value>],
        pos: usize,
    },
    Selected {
        rows: &'a [Vec<graphdb_core::Value>],
        indices: &'a [usize],
        pos: usize,
    },
}

impl<'a> Iterator for VisibleRows<'a> {
    type Item = &'a Vec<graphdb_core::Value>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            VisibleRows::All { rows, pos } => {
                let row = rows.get(*pos)?;
                *pos += 1;
                Some(row)
            }
            VisibleRows::Selected { rows, indices, pos } => {
                let &i = indices.get(*pos)?;
                *pos += 1;
                rows.get(i)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = match self {
            VisibleRows::All { rows, pos } => rows.len().saturating_sub(*pos),
            VisibleRows::Selected { indices, pos, .. } => indices.len().saturating_sub(*pos),
        };
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for VisibleRows<'_> {}

#[cfg(test)]
mod tests {
    use super::super::core::DataChunk;
    use crate::executor::streaming::slot::SlotLayout;
    use graphdb_core::Value;
    use std::sync::Arc;

    fn layout1() -> Arc<SlotLayout> {
        Arc::new(SlotLayout::from_names(&["a".to_string()]))
    }

    #[test]
    fn opaque_boundary_expands_multiplicity() {
        let rows = vec![vec![Value::Int(1)], vec![Value::Int(2)]];
        let mut chunk = DataChunk::new_with_layout(rows, layout1()).with_selection(vec![0, 1]);
        chunk.set_multiplicity(3);
        assert_eq!(chunk.logical_len(), 6);
        assert!(chunk.normalize_for_opaque("Sort"));
        assert_eq!(chunk.multiplicity(), 1);
        assert!(chunk.selection().is_none());
        assert_eq!(chunk.rows.len(), 6);
        assert_eq!(
            chunk.rows,
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(1)],
                vec![Value::Int(2)],
            ]
        );
    }

    #[test]
    fn opaque_boundary_noop_when_flat() {
        let rows = vec![vec![Value::Int(1)]];
        let mut chunk = DataChunk::new_with_layout(rows, layout1());
        assert!(!chunk.normalize_for_opaque("Sort"));
        assert_eq!(chunk.rows.len(), 1);
    }

    #[test]
    fn expand_multiplicity_drops_hidden_rows() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout1()).with_selection(vec![0, 2]);
        chunk.set_multiplicity(2);
        assert!(chunk.expand_multiplicity_in_place());
        assert_eq!(chunk.rows.len(), 4);
        assert_eq!(
            chunk.rows,
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(3)],
                vec![Value::Int(1)],
                vec![Value::Int(3)],
            ]
        );
    }

    #[test]
    fn multiplicity_expansion_runs_on_pooled_buffers() {
        use super::super::pool::{RowBufferPool, MAX_POOLED_ROWS};
        // Pre-fill the pool past its bound: only MAX buffers are retained.
        // The pool is process-global, so this test only asserts the upper
        // bound (concurrent tests may also touch the pool) plus the
        // deterministic expanded contents.
        RowBufferPool::clear_for_test();
        let mut spare: Vec<Vec<Value>> = (0..(MAX_POOLED_ROWS + 8))
            .map(|i| vec![Value::Int(i as i32)])
            .collect();
        RowBufferPool::release_rows(&mut spare);
        let rows = vec![vec![Value::Int(7)], vec![Value::Int(8)]];
        let mut chunk = DataChunk::new_with_layout(rows, layout1());
        chunk.set_multiplicity(3);
        assert!(chunk.expand_multiplicity_in_place());
        assert_eq!(
            chunk.rows,
            vec![
                vec![Value::Int(7)],
                vec![Value::Int(8)],
                vec![Value::Int(7)],
                vec![Value::Int(8)],
                vec![Value::Int(7)],
                vec![Value::Int(8)],
            ]
        );
        assert!(RowBufferPool::pool_len() <= MAX_POOLED_ROWS);
        RowBufferPool::clear_for_test();
    }
}

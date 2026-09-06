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
//!   merge; they assume `multiplicity == 1` and document it. A future
//!   multiplicity producer must expand at those boundaries with
//!   [`DataChunk::expand_visible_rows`] first.

use super::typed::gather_typed_column;
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
        self.typed_columns = None;
        self.multiplicity = 1;
        if multiplicity <= 1 || out.is_empty() {
            return out;
        }
        let total = (out.len() as u64).saturating_mul(multiplicity) as usize;
        let mut expanded = Vec::with_capacity(total.min(1 << 22));
        for _ in 0..multiplicity {
            expanded.extend(out.iter().cloned());
            if expanded.len() >= (1 << 22) {
                expanded.reserve(total.saturating_sub(expanded.len()).min(1 << 22));
            }
        }
        expanded
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

    /// Contract: materialize any attached selection (opaque-operator
    /// boundary). Returns `true` when a selection was actually materialized.
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

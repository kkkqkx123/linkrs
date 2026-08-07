//! Selection vectors, index-based take, and slice operations

use crate::query::executor::streaming::chunk::core::DataChunk;
use super::typed::gather_typed_column;

impl DataChunk {
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

    pub fn materialize_selection(&mut self) -> bool {
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
        if let Some(stats) = &self.columnar_stats {
            stats.record_selection_materialized();
        }
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
            schema,
            layout,
            memory_reservation: self.memory_reservation.take(),
            columnar_stats: self.columnar_stats.clone(),
        }
    }
}
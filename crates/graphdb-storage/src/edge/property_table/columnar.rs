//! Columnar integration: ColumnStore sync, projected reads, fast paths.

use super::*;

/// A batch of property rows, where each row is an optional list of
/// (property_name, optional_value) pairs.
pub(crate) type BatchPropertyRows = Vec<Option<Vec<(String, Option<Value>)>>>;

impl PropertyTable {
    pub fn column_values(&self, col_idx: usize) -> Vec<Option<Value>> {
        if col_idx >= self.schema.len() {
            return Vec::new();
        }
        let col_name = self.schema[col_idx].name.clone();
        if let Some(col) = self.column_store.get_column(&col_name) {
            let mut values = Vec::with_capacity(self.row_create_ts.len());
            for row_idx in 0..self.row_create_ts.len() {
                let offset = prop_index_to_offset(row_idx);
                if self.row_create_ts[row_idx] == 0
                    || self.free_list.contains(&offset)
                    || self.row_delete_ts.get(row_idx).and_then(|v| *v).is_some()
                {
                    values.push(None);
                } else {
                    values.push(col.get(row_idx));
                }
            }
            if !values.is_empty() {
                return values;
            }
        }
        debug_assert!(
            false,
            "column_values: columnar store missing column '{col_name}'"
        );
        Vec::new()
    }

    /// Column pruning: read only `projection` columns for one row at `query_ts`.
    pub fn get_projected(
        &self,
        offset: u32,
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let row_idx = prop_offset_to_index(offset)?;
        if row_idx >= self.row_create_ts.len() {
            return None;
        }
        if !self.is_row_visible(row_idx, offset, query_ts) {
            return None;
        }
        if projection.is_empty() {
            return self.get(offset, query_ts);
        }
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        let mut out = Vec::with_capacity(projection.len());
        for col_name in projection {
            if let Some(col) = self.column_store.get_column(col_name) {
                let val = col.get_at_ts(row_idx, ts);
                out.push((col_name.clone(), val));
            } else {
                if let Some(row) = self.get(offset, query_ts) {
                    if let Some((_, v)) = row.into_iter().find(|(n, _)| n == col_name) {
                        out.push((col_name.clone(), v));
                    } else {
                        out.push((col_name.clone(), None));
                    }
                } else {
                    out.push((col_name.clone(), None));
                }
            }
        }
        Some(out)
    }

    pub fn get_projected_batch(
        &self,
        offsets: &[u32],
        projection: &[String],
        query_ts: Option<Timestamp>,
    ) -> Vec<ProjectedRow> {
        let ts = query_ts.unwrap_or(Timestamp::MAX);
        let mut out = Vec::with_capacity(offsets.len());
        for &off in offsets {
            out.push(self.get_projected(off, projection, query_ts));
        }
        let row_indices: Vec<usize> = offsets
            .iter()
            .filter_map(|o| prop_offset_to_index(*o))
            .collect();
        if !projection.is_empty() && !row_indices.is_empty() {
            let _ = self
                .column_store
                .get_projected_batch_at_ts(&row_indices, projection, ts);
        }
        out
    }

    pub fn apply_column_encoding(
        &mut self,
        col_name: &str,
        encoding: EncodingType,
    ) -> StorageResult<()> {
        self.column_store
            .apply_encoding_to_column(col_name, encoding, 4096)
    }

    #[inline]
    pub fn prefetch(&self, _offset: u32) {
        // Columnar prefetch is a no-op; column data is already cache-friendly.
    }

    #[inline]
    pub fn prefetch_batch(&self, _offsets: &[u32]) {}

    pub fn get_fast(
        &self,
        offset: u32,
        query_ts: Option<Timestamp>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        self.get(offset, query_ts)
    }

    pub fn get_batch<'a, I>(&'a self, offsets: I, query_ts: Option<Timestamp>) -> BatchPropertyRows
    where
        I: IntoIterator<Item = &'a u32>,
    {
        let offsets: Vec<_> = offsets.into_iter().collect();
        let mut indexed: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(idx, offset)| (idx, **offset))
            .collect();
        indexed.sort_by_key(|(_, offset)| *offset);
        let sorted_results: Vec<_> = indexed
            .iter()
            .map(|(_, offset)| self.get_fast(*offset, query_ts))
            .collect();
        let mut results = vec![None; offsets.len()];
        for (orig_idx, sorted_result) in indexed.iter().zip(sorted_results) {
            results[orig_idx.0] = sorted_result;
        }
        results
    }
}

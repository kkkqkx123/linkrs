use super::CsrWithProperties;
use graphdb_core::{StorageError, StorageResult, Value};

impl CsrWithProperties {
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        let col = self
            .column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))?;
        let mut min: Option<Value> = None;
        let mut max: Option<Value> = None;
        for zone in col.zone_maps() {
            if let Some(v) = &zone.min {
                match &min {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Greater => {}
                    _ => min = Some(v.clone()),
                }
            }
            if let Some(v) = &zone.max {
                match &max {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Less => {}
                    _ => max = Some(v.clone()),
                }
            }
        }
        let persisted = col.stats();
        let null_count = persisted.map(|s| s.null_count);
        let (distinct_count, hll) = match persisted.and_then(|s| s.hll.clone()) {
            Some(h) => {
                let est = h.estimate();
                (Some(est), persisted.and_then(|s| s.hll.clone()))
            }
            None => (None, None),
        };
        Some(crate::stats_reader::ColumnStatsSnapshot {
            row_count: self.row_count as u64,
            null_count,
            distinct_count,
            hll,
            min_value: min,
            max_value: max,
        })
    }

    /// Global min/max bounds of one column for predicate pushdown pruning.
    ///
    /// Merges the per-chunk zone bounds with the same comparison used to
    /// widen them. Bounds only ever widen (rebuilds never shrink), so they
    /// contain every non-null value any snapshot can still observe through
    /// a version chain, and pruning against them is sound for any snapshot
    /// timestamp. `None` when the column is absent or holds no recorded
    /// bounds (all-null columns included): callers must scan instead of
    /// pruning.
    pub fn prune_bounds(&self, column: &str) -> Option<(Value, Value)> {
        let col = self
            .column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))?;
        let mut min: Option<Value> = None;
        let mut max: Option<Value> = None;
        for zone in col.zone_maps() {
            if let Some(v) = &zone.min {
                match &min {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Greater => {}
                    _ => min = Some(v.clone()),
                }
            }
            if let Some(v) = &zone.max {
                match &max {
                    Some(cur)
                        if crate::vertex::column::compare_values(cur, v)
                            != std::cmp::Ordering::Less => {}
                    _ => max = Some(v.clone()),
                }
            }
        }
        match (min, max) {
            (Some(lower), Some(upper)) => Some((lower, upper)),
            _ => None,
        }
    }

    /// Encoding applied to one property column, if the column exists.
    pub fn column_encoding_type(&self, column: &str) -> Option<crate::encoding::EncodingType> {
        self.column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))
            .map(|c| c.encoding_type())
    }

    /// Apply one encoding to a single property column.
    ///
    /// Chunked columns take the chunk-local path so point updates keep
    /// decoding only the affected chunk; plain columns dispatch by type.
    /// Empty columns are a no-op. Unknown columns are an explicit error.
    pub fn apply_encoding_to_column(
        &mut self,
        column: &str,
        encoding_type: crate::encoding::EncodingType,
        fsst_max_symbols: usize,
    ) -> StorageResult<()> {
        let idx = self
            .column_index
            .get(column)
            .copied()
            .ok_or_else(|| StorageError::column_not_found(column.to_string()))?;
        let col = &mut self.property_columns[idx];
        if col.is_empty() {
            return Ok(());
        }
        if col.has_chunks() {
            col.apply_encoding_to_chunks(encoding_type, fsst_max_symbols)?;
            if let Some(schema) = self.property_schema.get_mut(idx) {
                schema.encoding_type = col.encoding_type();
            }
            return Ok(());
        }
        match encoding_type {
            crate::encoding::EncodingType::Fsst => {
                col.apply_fsst_encoding(fsst_max_symbols)?;
            }
            crate::encoding::EncodingType::Dictionary => {
                col.apply_dictionary_encoding()?;
            }
            crate::encoding::EncodingType::Rle => {
                col.apply_rle_encoding()?;
            }
            crate::encoding::EncodingType::BitPacking => {
                col.apply_bitpacking_encoding()?;
            }
            crate::encoding::EncodingType::Alp => {
                col.apply_alp_encoding()?;
            }
            crate::encoding::EncodingType::Constant => {
                col.apply_constant_encoding()?;
            }
            crate::encoding::EncodingType::None => {}
        }
        if let Some(schema) = self.property_schema.get_mut(idx) {
            schema.encoding_type = col.encoding_type();
        }
        Ok(())
    }

    /// Select and apply one encoding per property column from current values.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between runs
    /// by design so everyday writes never pay re-encoding. Returns the number
    /// of columns that received an encoding.
    pub fn auto_encode_properties(&mut self) -> usize {
        let selector = crate::encoding::EncodingSelector::default();
        let mut encoded = 0usize;
        for idx in 0..self.property_columns.len() {
            let data_type = self.property_columns[idx].data_type.clone();
            let values: Vec<Option<Value>> = (0..self.property_columns[idx].len())
                .map(|row| self.property_columns[idx].get(row))
                .collect();
            if values.is_empty() {
                continue;
            }
            let selected = selector.select_for_column(&data_type, &values);
            if selected == crate::encoding::EncodingType::None {
                continue;
            }
            let name = self.property_columns[idx].name.clone();
            if self
                .apply_encoding_to_column(name.as_str(), selected, 255)
                .is_ok()
            {
                encoded += 1;
            }
        }
        encoded
    }

    /// Infer encodings for checkpoint dirty columns from current values.
    ///
    /// Checkpoint-only entry: the write path never calls this, so hot
    /// columns pay no re-encoding on everyday writes. `dirty_only` selects
    /// the inference set; `None` infers every column for first-flush
    /// completeness. Columns below `min_rows` are skipped. A column keeps
    /// its current encoding when inference yields none or matches. Returns
    /// the number of columns that changed encoding.
    pub fn adapt_encodings_for_checkpoint(
        &mut self,
        dirty_only: Option<&[String]>,
        min_rows: usize,
    ) -> usize {
        let selector = crate::encoding::EncodingSelector::default();
        let targets: Vec<(usize, String)> = match dirty_only {
            Some(names) if !names.is_empty() => names
                .iter()
                .filter_map(|name| self.column_index.get(name).map(|idx| (*idx, name.clone())))
                .collect(),
            _ => self
                .property_columns
                .iter()
                .map(|col| col.name.clone())
                .enumerate()
                .collect(),
        };
        let mut changed = 0usize;
        for (idx, name) in targets {
            let Some(col) = self.property_columns.get(idx) else {
                continue;
            };
            if col.len() < min_rows || col.is_empty() {
                continue;
            }
            let data_type = col.data_type.clone();
            let current = col.encoding_type();
            let values: Vec<Option<Value>> = (0..col.len()).map(|row| col.get(row)).collect();
            if values.is_empty() {
                continue;
            }
            let selected = selector.select_for_column(&data_type, &values);
            if selected == crate::encoding::EncodingType::None || selected == current {
                continue;
            }
            if self
                .apply_encoding_to_column(name.as_str(), selected, 255)
                .is_ok()
            {
                changed += 1;
            }
        }
        changed
    }
}

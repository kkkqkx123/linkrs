use super::{CsrWithProperties, FSST_MAX_SYMBOLS};
use graphdb_core::{StorageError, StorageResult, Value};
use std::sync::Arc;

impl CsrWithProperties {
    /// Merge the per-chunk zone bounds of one property column.
    ///
    /// Bounds only ever widen (rebuilds never shrink), so the result contains
    /// every non-null value any snapshot can still observe through a version
    /// chain. `None` when the column is absent or holds no recorded bounds
    /// (all-null columns included).
    fn zone_bounds(&self, column: &str) -> Option<(Value, Value)> {
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

    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        let col = self
            .column_index
            .get(column)
            .and_then(|&idx| self.property_columns.get(idx))?;
        let bounds = self.zone_bounds(column);
        let persisted = col.stats();
        let null_count = persisted.as_ref().map(|s| s.null_count);
        let (distinct_count, hll) = match persisted.as_ref().and_then(|s| s.hll.clone()) {
            Some(h) => {
                let est = h.estimate();
                (Some(est), persisted.as_ref().and_then(|s| s.hll.clone()))
            }
            None => (None, None),
        };
        Some(crate::stats_reader::ColumnStatsSnapshot {
            row_count: self.row_count as u64,
            null_count,
            distinct_count,
            hll,
            min_value: bounds.as_ref().map(|(lower, _)| lower.clone()),
            max_value: bounds.as_ref().map(|(_, upper)| upper.clone()),
        })
    }

    /// Global min/max bounds of one column for predicate pushdown pruning.
    ///
    /// Callers must scan instead of pruning when this is `None`.
    pub fn prune_bounds(&self, column: &str) -> Option<(Value, Value)> {
        self.zone_bounds(column)
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
    /// Property columns are chunk-backed, so the encoding is applied per
    /// chunk and point updates keep decoding only the affected chunk.
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
        col.apply_encoding_to_chunks(encoding_type, fsst_max_symbols)?;
        if let Some(schema) = self.property_schema.get_mut(idx) {
            schema.encoding_type = col.encoding_type();
        }
        Ok(())
    }

    /// Select and apply one encoding per property column.
    ///
    /// Explicit maintenance operation: hot columns stay unencoded between runs
    /// by design so everyday writes never pay re-encoding. Each column is
    /// profiled chunk by chunk, so no column-wide value vector is built.
    /// Returns the number of columns that received an encoding.
    pub fn auto_encode_properties(&mut self) -> usize {
        let targets = self.all_column_targets();
        self.encode_targets(&targets, 0)
    }

    /// Infer encodings for checkpoint dirty columns.
    ///
    /// Checkpoint-only entry: the write path never calls this, so hot columns
    /// pay no re-encoding on everyday writes. `dirty_only` selects the
    /// inference set; `None` infers every column for first-flush
    /// completeness. Columns below `min_rows` are skipped. A column keeps
    /// its current encoding when inference yields none or matches. Returns
    /// the number of columns that changed encoding.
    pub fn adapt_encodings_for_checkpoint(
        &mut self,
        dirty_only: Option<&[Arc<str>]>,
        min_rows: usize,
    ) -> usize {
        let targets = match dirty_only {
            Some(names) if !names.is_empty() => names
                .iter()
                .filter_map(|name| {
                    self.column_index
                        .get(&**name)
                        .map(|idx| (*idx, name.clone()))
                })
                .collect(),
            _ => self.all_column_targets(),
        };
        self.encode_targets(&targets, min_rows)
    }

    /// Every schema column as an encode target, in schema order.
    fn all_column_targets(&self) -> Vec<(usize, Arc<str>)> {
        self.property_schema
            .iter()
            .enumerate()
            .map(|(idx, schema)| (idx, schema.name.clone()))
            .collect()
    }

    /// Profile each target column chunk by chunk and apply the winner.
    ///
    /// The single place where a column's encoding is inferred, so explicit
    /// maintenance and checkpoint adaptation can never drift apart. Columns
    /// under `min_rows`, empty columns and columns whose winner matches the
    /// encoding they already carry are left alone.
    fn encode_targets(&mut self, targets: &[(usize, Arc<str>)], min_rows: usize) -> usize {
        let selector = crate::encoding::EncodingSelector::default();
        let mut encoded = 0usize;
        for (idx, name) in targets {
            let Some(col) = self.property_columns.get(*idx) else {
                continue;
            };
            if col.len() < min_rows || col.is_empty() {
                continue;
            }
            let selected = col.select_encoding(&selector);
            if selected == crate::encoding::EncodingType::None || selected == col.encoding_type() {
                continue;
            }
            if self
                .apply_encoding_to_column(name, selected, FSST_MAX_SYMBOLS)
                .is_ok()
            {
                encoded += 1;
            }
        }
        encoded
    }
}

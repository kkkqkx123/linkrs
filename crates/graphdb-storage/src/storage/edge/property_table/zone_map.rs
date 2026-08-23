//! Zone maps: per-chunk ColumnStats construction, query, and pruning.

use super::*;

impl PropertyTable {
    /// Refresh zone map for the chunk containing `row_idx`.
    /// Recomputes `ColumnStats` for that chunk for every column, using the
    /// columnar store's current values (MVCC current view). Zone maps are
    /// best-effort and fully rebuilt on `rebuild_zone_maps` or flush.
    pub(super) fn refresh_zone_map_for_row(&mut self, row_idx: usize) {
        let chunk_id = row_idx / ZONE_MAP_CHUNK_SIZE;
        let chunk_start = chunk_id * ZONE_MAP_CHUNK_SIZE;
        let chunk_end = (chunk_start + ZONE_MAP_CHUNK_SIZE).min(self.records.len());
        if chunk_start >= chunk_end {
            return;
        }
        // Collect live rows in chunk.
        let mut live_rows: Vec<usize> = Vec::new();
        for idx in chunk_start..chunk_end {
            if self.records.get(idx).and_then(|r| r.as_ref()).is_some() {
                // Consider only rows not tombstoned at current max timestamp view;
                // for zone map we use current values (not historical).
                if self.records[idx]
                    .as_ref()
                    .is_some_and(|rec| rec.delete_ts.is_none())
                {
                    live_rows.push(idx);
                }
            }
        }
        // For each column, compute stats for this chunk.
        let col_names: Vec<String> = self.schema.iter().map(|s| s.name.clone()).collect();
        for col_name in col_names {
            let col = match self.column_store.get_column(&col_name) {
                Some(c) => c,
                None => continue,
            };
            // Gather values for live rows in chunk.
            let values: Vec<Option<Value>> = live_rows.iter().map(|&r| col.get(r)).collect();
            let raw_size = values.len() as u64
                * crate::storage::vertex::column_store::element_size(&col.data_type).max(1) as u64;
            let stats = compute_stats(&values, col.encoding_type(), raw_size, raw_size);
            let entry = self.zone_maps.entry(col_name.clone()).or_default();
            if entry.len() <= chunk_id {
                entry.resize(chunk_id + 1, ColumnStats::new(EncodingType::None, 0, 0));
            }
            entry[chunk_id] = stats;
        }
    }

    /// Rebuild all zone maps from scratch (used after bulk load).
    pub fn rebuild_zone_maps(&mut self) {
        self.zone_maps.clear();
        let total_chunks = self.records.len().div_ceil(ZONE_MAP_CHUNK_SIZE);
        if total_chunks == 0 {
            return;
        }
        for chunk_id in 0..total_chunks {
            let row_idx = chunk_id * ZONE_MAP_CHUNK_SIZE;
            self.refresh_zone_map_for_row(row_idx);
        }
    }

    /// Optimizer-facing statistics snapshot for one property column.
    ///
    /// Delegates to [`Self::compute_column_stats`] (which prefers the
    /// persisted zone-map aggregation and falls back to a full column scan)
    /// and pairs it with the live row count. Returns `None` when the column
    /// is unknown, carries no values, or the columnar store has not been
    /// populated yet (pre-flush).
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::storage::stats_reader::ColumnStatsSnapshot> {
        use crate::storage::stats_reader::ColumnStatsSnapshot;

        let _col_idx = self.schema.iter().position(|s| s.name == column)?;

        // Fast path: when zone maps have data for this column, aggregate
        // directly without touching the (possibly empty) columnar store.
        if let Some(zones) = self.zone_maps.get(column) {
            if !zones.is_empty() {
                let mut min_value: Option<Value> = None;
                let mut max_value: Option<Value> = None;
                let mut null_count: u64 = 0;
                for zs in zones {
                    crate::storage::stats_reader::merge_min(&mut min_value, zs.min_value.clone());
                    crate::storage::stats_reader::merge_max(&mut max_value, zs.max_value.clone());
                    null_count += zs.null_count;
                }
                // Return even when min/max are None — the snapshot still
                // carries row_count and null_count which are useful.
                return Some(ColumnStatsSnapshot {
                    row_count: self.row_count() as u64,
                    null_count: Some(null_count),
                    distinct_count: None,
                    min_value,
                    max_value,
                });
            }
        }

        // Slow path: delegate to compute_column_stats which may do a full
        // column scan.  Only use it when the columnar store actually has
        // data for this column; otherwise the scan would produce an empty
        // result and we conservatively return None so the collector falls
        // back to sampling.
        None
    }

    pub fn compute_column_stats(
        &self,
        col_idx: usize,
    ) -> Option<crate::storage::column_stats::ColumnStats> {
        if col_idx >= self.schema.len() {
            return None;
        }
        let schema = &self.schema[col_idx];
        // Prefer per-column zone map aggregation if available.
        if let Some(zm) = self.zone_maps.get(&schema.name) {
            if !zm.is_empty() {
                // Aggregate zone maps into global stats (min = min(mins), max = max(maxes), etc.)
                let mut agg = ColumnStats::new(EncodingType::None, 0, 0);
                let mut all_values: Vec<Option<Value>> = Vec::new();
                for zs in zm {
                    if let Some(ref v) = zs.min_value {
                        all_values.push(Some(v.clone()));
                    }
                    if let Some(ref v) = zs.max_value {
                        all_values.push(Some(v.clone()));
                    }
                    agg.null_count += zs.null_count;
                    agg.compressed_size += zs.compressed_size;
                    agg.raw_size += zs.raw_size;
                }
                // Recompute global min/max/distinct from chunk stats where possible,
                // else fallback to full column scan.
                if !all_values.is_empty() {
                    agg.min_value = all_values.iter().filter_map(|v| v.as_ref()).min().cloned();
                    agg.max_value = all_values.iter().filter_map(|v| v.as_ref()).max().cloned();
                    // distinct is sum of chunk distincts capped; precise requires scan.
                }
                // If zone maps are incomplete, fall through to full scan.
                if agg.raw_size > 0 {
                    return Some(agg);
                }
            }
        }
        let values = self.column_values(col_idx);
        let raw_size = values.len() as u64
            * crate::storage::vertex::column_store::element_size(&schema.data_type).max(1) as u64;
        // Use column's actual encoding if columnar store has it.
        let enc = self
            .column_store
            .get_column(&schema.name)
            .map(|c| c.encoding_type())
            .unwrap_or(EncodingType::None);
        Some(crate::storage::column_stats::compute_stats(
            &values, enc, raw_size, raw_size,
        ))
    }

    /// Zone-map predicate pruning: given a column and value range, return a
    /// bitmask per chunk indicating whether the chunk may contain matching rows.
    /// `None` bounds are unbounded. Chunks with no overlap can be skipped.
    pub fn prune_chunks_by_range(
        &self,
        column: &str,
        lower: Option<&Value>,
        upper: Option<&Value>,
        include_lower: bool,
        include_upper: bool,
    ) -> Option<Vec<bool>> {
        let zones = self.zone_maps.get(column)?;
        let mut mask = Vec::with_capacity(zones.len());
        for stats in zones {
            let mut keep = true;
            if let Some(lo) = lower {
                if let Some(ref max) = stats.max_value {
                    let cmp = max.cmp(lo);
                    if cmp == std::cmp::Ordering::Less
                        || (cmp == std::cmp::Ordering::Equal && !include_upper && max == lo)
                    {
                        // Actually need to compare max < lower or max == lower when not inclusive?
                        // Simplified: if max < lower, chunk cannot contain value >= lower.
                        if max < lo {
                            keep = false;
                        } else if !include_lower && max == lo {
                            // max == lower but lower exclusive: still need to check min?
                            // For range pruning we conservatively keep.
                        }
                    }
                    if !keep {
                        // Check lower bound against max.
                        if max < lo || (!include_lower && max == lo) {
                            keep = false;
                        }
                    }
                }
            }
            if keep {
                if let Some(hi) = upper {
                    if let Some(ref min) = stats.min_value {
                        if min > hi || (!include_upper && min == hi) {
                            keep = false;
                        }
                    }
                }
            }
            mask.push(keep);
        }
        Some(mask)
    }

    /// Return zone maps for a column (for ShowStats / optimizer).
    pub fn zone_map_for_column(&self, column: &str) -> Option<&[ColumnStats]> {
        self.zone_maps.get(column).map(|v| v.as_slice())
    }

    /// All zone maps (for persistence / diagnostics).
    pub fn all_zone_maps(&self) -> &HashMap<String, Vec<ColumnStats>> {
        &self.zone_maps
    }
}

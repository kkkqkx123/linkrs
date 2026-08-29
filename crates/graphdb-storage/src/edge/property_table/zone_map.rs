//! Zone maps: per-chunk ColumnStats construction, query, and pruning.

use super::*;

impl PropertyTable {
    pub(super) fn refresh_zone_map_for_row(&mut self, row_idx: usize) {
        let chunk_id = row_idx / ZONE_MAP_CHUNK_SIZE;
        let chunk_start = chunk_id * ZONE_MAP_CHUNK_SIZE;
        let total_rows = self.row_create_ts.len();
        let chunk_end = (chunk_start + ZONE_MAP_CHUNK_SIZE).min(total_rows);
        if chunk_start >= chunk_end {
            return;
        }
        let mut live_rows: Vec<usize> = Vec::new();
        for idx in chunk_start..chunk_end {
            if self.row_create_ts.get(idx).copied().unwrap_or(0) == 0 {
                continue;
            }
            let offset = prop_index_to_offset(idx);
            if self.free_list.contains(&offset) {
                continue;
            }
            if self.row_delete_ts.get(idx).and_then(|v| *v).is_some() {
                continue;
            }
            live_rows.push(idx);
        }
        let col_names: Vec<String> = self.schema.iter().map(|s| s.name.clone()).collect();
        for col_name in col_names {
            let col = match self.column_store.get_column(&col_name) {
                Some(c) => c,
                None => continue,
            };
            let values: Vec<Option<Value>> = live_rows.iter().map(|&r| col.get(r)).collect();
            let raw_size = values.len() as u64
                * crate::vertex::column_store::element_size(&col.data_type).max(1) as u64;
            let stats = compute_stats(&values, col.encoding_type(), raw_size, raw_size);
            let entry = self.zone_maps.entry(col_name.clone()).or_default();
            if entry.len() <= chunk_id {
                entry.resize(chunk_id + 1, ColumnStats::new(EncodingType::None, 0, 0));
            }
            entry[chunk_id] = stats;
        }
    }

    pub fn rebuild_zone_maps(&mut self) {
        self.zone_maps.clear();
        let total_chunks = self.row_create_ts.len().div_ceil(ZONE_MAP_CHUNK_SIZE);
        if total_chunks == 0 {
            return;
        }
        for chunk_id in 0..total_chunks {
            let row_idx = chunk_id * ZONE_MAP_CHUNK_SIZE;
            self.refresh_zone_map_for_row(row_idx);
        }
    }

    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        use crate::stats_reader::ColumnStatsSnapshot;

        let _col_idx = self.schema.iter().position(|s| s.name == column)?;

        if let Some(zones) = self.zone_maps.get(column) {
            if !zones.is_empty() {
                let mut min_value: Option<Value> = None;
                let mut max_value: Option<Value> = None;
                let mut null_count: u64 = 0;
                for zs in zones {
                    crate::stats_reader::merge_min(&mut min_value, zs.min_value.clone());
                    crate::stats_reader::merge_max(&mut max_value, zs.max_value.clone());
                    null_count += zs.null_count;
                }
                return Some(ColumnStatsSnapshot {
                    row_count: self.row_count() as u64,
                    null_count: Some(null_count),
                    distinct_count: None,
                    min_value,
                    max_value,
                });
            }
        }

        None
    }

    pub fn compute_column_stats(&self, col_idx: usize) -> Option<crate::column_stats::ColumnStats> {
        if col_idx >= self.schema.len() {
            return None;
        }
        let schema = &self.schema[col_idx];
        if let Some(zm) = self.zone_maps.get(&schema.name) {
            if !zm.is_empty() {
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
                if !all_values.is_empty() {
                    agg.min_value = all_values.iter().filter_map(|v| v.as_ref()).min().cloned();
                    agg.max_value = all_values.iter().filter_map(|v| v.as_ref()).max().cloned();
                }
                if agg.raw_size > 0 {
                    return Some(agg);
                }
            }
        }
        let values = self.column_values(col_idx);
        let raw_size = values.len() as u64
            * crate::vertex::column_store::element_size(&schema.data_type).max(1) as u64;
        let enc = self
            .column_store
            .get_column(&schema.name)
            .map(|c| c.encoding_type())
            .unwrap_or(EncodingType::None);
        Some(crate::column_stats::compute_stats(
            &values, enc, raw_size, raw_size,
        ))
    }

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
                        if max < lo {
                            keep = false;
                        } else if !include_lower && max == lo {
                        }
                    }
                    if !keep {
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

    pub fn zone_map_for_column(&self, column: &str) -> Option<&[ColumnStats]> {
        self.zone_maps.get(column).map(|v| v.as_slice())
    }

    pub fn all_zone_maps(&self) -> &HashMap<String, Vec<ColumnStats>> {
        &self.zone_maps
    }
}

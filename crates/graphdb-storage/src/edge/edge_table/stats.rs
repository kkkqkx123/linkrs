//! Statistics structures for observability and monitoring.
//!
//! Provides statistics for tombstones and deletions to help track
//! node-group sharded edge table behavior.

use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

use std::collections::HashMap;

use crate::column_stats::{compute_stats_streaming, deserialize_stat_value, serialize_stat_value};
use crate::encoding::EncodingType;

/// Statistics about tombstones for observability and debugging.
#[derive(Debug, Clone)]
pub struct TombstoneStats {
    /// Number of active tombstones
    pub count: usize,
    /// Approximate memory used by tombstones (bytes)
    pub memory_bytes: usize,
    /// Oldest deletion timestamp in tombstones
    pub oldest_delete_ts: Option<Timestamp>,
    /// Newest deletion timestamp in tombstones
    pub newest_delete_ts: Option<Timestamp>,
}

impl TombstoneStats {
    /// Estimate memory usage per authority record.
    ///
    /// One record holds the edge id plus its create/delete stamps inside a
    /// hash node. The estimate adds a fixed per-node hash overhead on top of
    /// the entry payload so backpressure and maintenance triggers see the
    /// map cost instead of a bare key size.
    pub fn estimate_memory(count: usize) -> usize {
        const HASH_NODE_OVERHEAD: usize = 48;
        const AUTHORITY_RECORD: usize =
            std::mem::size_of::<EdgeId>() + 2 * std::mem::size_of::<Timestamp>();
        count * (AUTHORITY_RECORD + HASH_NODE_OVERHEAD)
    }
}

/// Statistics about deletions in the sharded table for observability.
///
/// Tracks the deletion share over tracked edges. This is a separate dimension
/// from the wasted-share fragmentation ratio: deletion share drives freeze
/// interest while fragmentation waste drives group merges. The two must never
/// share a threshold.
#[derive(Debug, Clone, Default)]
pub struct DeletionStats {
    /// Total edges deleted and still tracked
    pub total_deleted_edges: u64,
    /// Total live edges (for percentage calculation)
    pub total_live_edges: u64,
}

impl DeletionStats {
    /// Get deletion share as a ratio (0.0 to 1.0) over all tracked edges.
    pub fn deletion_ratio(&self) -> f64 {
        let total = self
            .total_live_edges
            .saturating_add(self.total_deleted_edges);
        if total == 0 {
            0.0
        } else {
            self.total_deleted_edges as f64 / total as f64
        }
    }

    /// Get deletion percentage (0-100)
    pub fn deletion_percentage(&self) -> f64 {
        self.deletion_ratio() * 100.0
    }

    /// Check if deletions are significant (>10%)
    pub fn is_significant(&self) -> bool {
        self.deletion_ratio() > 0.1
    }
}

/// Wire version of one group segment-statistics record. Version 1 carries
/// row counts plus sort bounds and per-column statistics; older payloads are
/// rejected, never converted.
pub const SEGMENT_STATS_RECORD_VERSION: u32 = 1;

/// Per-group segment statistics for scan pruning.
///
/// One record per owner group: row and live counts, the property null-cell
/// total, endpoint sort bounds, and per-column statistics. Column statistics
/// reuse the attribute-column caliber exactly: they are built with
/// `compute_stats_streaming` and serialized with `ColumnStats` metadata, so
/// no second statistics structure exists. Bounds only ever widen across
/// refreshes so pruning stays conservative for every snapshot; counts are
/// exact-current for observability. Topology itself never holds nulls, so a
/// zero null total with bitmap-backed property nulls is the normal case.
#[derive(Debug, Clone)]
pub struct GroupSegmentStats {
    pub group: u32,
    pub row_count: u64,
    pub live_count: u64,
    pub null_count: u64,
    pub sort_min: Option<Value>,
    pub sort_max: Option<Value>,
    pub columns: HashMap<String, crate::column_stats::ColumnStats>,
}

impl GroupSegmentStats {
    /// Collect fresh statistics for one group from its endpoint values and
    /// per-column value slices. The endpoint column uses the same streaming
    /// aggregation as attribute columns.
    pub fn collect(
        group: u32,
        row_count: u64,
        live_count: u64,
        endpoints: &[u32],
        column_values: &HashMap<String, Vec<Option<Value>>>,
        column_encodings: &HashMap<String, EncodingType>,
    ) -> Self {
        let endpoint_values = endpoints
            .iter()
            .map(|endpoint| Some(Value::BigInt(*endpoint as i64)));
        let endpoint_stats = compute_stats_streaming(endpoint_values, EncodingType::None, 0, 0);
        let (sort_min, sort_max) = (
            endpoint_stats.min_value.clone(),
            endpoint_stats.max_value.clone(),
        );
        let mut null_count = 0u64;
        let mut columns = HashMap::with_capacity(column_values.len());
        for (name, values) in column_values {
            let encoding = column_encodings
                .get(name)
                .copied()
                .unwrap_or(EncodingType::None);
            let stats = compute_stats_streaming(values.iter().cloned(), encoding, 0, 0);
            null_count = null_count.saturating_add(stats.null_count);
            columns.insert(name.clone(), stats);
        }
        Self {
            group,
            row_count,
            live_count,
            null_count,
            sort_min,
            sort_max,
            columns,
        }
    }

    /// Merge fresh statistics into these bounds, widening only.
    ///
    /// Minimums move down, maximums move up, distinct estimates merge; counts
    /// take the fresh values. A group that once held a value keeps covering
    /// it, so segment pruning stays conservative for every snapshot while
    /// counts stay exact-current for observability.
    pub fn widen_with(&mut self, fresh: &GroupSegmentStats) {
        self.row_count = fresh.row_count;
        self.live_count = fresh.live_count;
        self.null_count = fresh.null_count;
        match (&self.sort_min, &fresh.sort_min) {
            (_, None) => {}
            (None, Some(_)) => self.sort_min = fresh.sort_min.clone(),
            (Some(current), Some(next)) => {
                if crate::cursor::compare_stat_values(next, current) == std::cmp::Ordering::Less {
                    self.sort_min = fresh.sort_min.clone();
                }
            }
        }
        match (&self.sort_max, &fresh.sort_max) {
            (_, None) => {}
            (None, Some(_)) => self.sort_max = fresh.sort_max.clone(),
            (Some(current), Some(next)) => {
                if crate::cursor::compare_stat_values(next, current) == std::cmp::Ordering::Greater
                {
                    self.sort_max = fresh.sort_max.clone();
                }
            }
        }
        for (name, fresh_stats) in &fresh.columns {
            match self.columns.get_mut(name) {
                None => {
                    self.columns.insert(name.clone(), fresh_stats.clone());
                }
                Some(current) => {
                    match (&current.min_value, &fresh_stats.min_value) {
                        (_, None) => {}
                        (None, Some(_)) => current.min_value = fresh_stats.min_value.clone(),
                        (Some(a), Some(b)) => {
                            if crate::cursor::compare_stat_values(b, a) == std::cmp::Ordering::Less
                            {
                                current.min_value = fresh_stats.min_value.clone();
                            }
                        }
                    }
                    match (&current.max_value, &fresh_stats.max_value) {
                        (_, None) => {}
                        (None, Some(_)) => current.max_value = fresh_stats.max_value.clone(),
                        (Some(a), Some(b)) => {
                            if crate::cursor::compare_stat_values(b, a)
                                == std::cmp::Ordering::Greater
                            {
                                current.max_value = fresh_stats.max_value.clone();
                            }
                        }
                    }
                    current.null_count = fresh_stats.null_count;
                    match (&current.hll, &fresh_stats.hll) {
                        (Some(_), Some(fresh_hll)) => {
                            if let Some(merged) = current.hll.clone() {
                                let mut combined = merged;
                                combined.merge(fresh_hll);
                                current.distinct_count = Some(combined.estimate());
                                current.hll = Some(combined);
                            } else {
                                current.hll = fresh_stats.hll.clone();
                                current.distinct_count = fresh_stats.distinct_count;
                            }
                        }
                        (None, Some(_)) => {
                            current.hll = fresh_stats.hll.clone();
                            current.distinct_count = fresh_stats.distinct_count;
                        }
                        _ => {
                            current.distinct_count = fresh_stats.distinct_count;
                        }
                    }
                    current.encoding_type = fresh_stats.encoding_type;
                }
            }
        }
    }

    /// Whether this segment may contain rows matching the pushed predicates.
    ///
    /// Conservative pre-filter: returns false only when a merged predicate
    /// range provably excludes the widened column bounds. Missing statistics
    /// or missing bounds never prune. Null-only reasoning never prunes
    /// either: nulls use bitmap semantics and old snapshots may still hold
    /// values where the flushed epoch shows nulls.
    pub fn may_contain(&self, predicates: &[crate::cursor::ScanPredicate]) -> bool {
        if predicates.is_empty() {
            return true;
        }
        for range in crate::cursor::ScanPredicate::merged_ranges(predicates) {
            let Some(stats) = self.columns.get(&range.column) else {
                continue;
            };
            let (Some(min), Some(max)) = (stats.min_value.as_ref(), stats.max_value.as_ref())
            else {
                continue;
            };
            if !range.overlaps(min, max) {
                return false;
            }
        }
        true
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&SEGMENT_STATS_RECORD_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group.to_le_bytes());
        out.extend_from_slice(&self.row_count.to_le_bytes());
        out.extend_from_slice(&self.live_count.to_le_bytes());
        out.extend_from_slice(&self.null_count.to_le_bytes());
        out.push(self.sort_min.is_some() as u8);
        if let Some(ref value) = self.sort_min {
            let mut buf = Vec::new();
            if serialize_stat_value(&mut buf, value).is_ok() {
                out.extend_from_slice(&(buf.len() as u32).to_le_bytes());
                out.extend_from_slice(&buf);
            } else {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        out.push(self.sort_max.is_some() as u8);
        if let Some(ref value) = self.sort_max {
            let mut buf = Vec::new();
            if serialize_stat_value(&mut buf, value).is_ok() {
                out.extend_from_slice(&(buf.len() as u32).to_le_bytes());
                out.extend_from_slice(&buf);
            } else {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        let mut names: Vec<&String> = self.columns.keys().collect();
        names.sort();
        out.extend_from_slice(&(names.len() as u32).to_le_bytes());
        for name in names {
            let bytes = name.as_bytes();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
            let mut stats_buf = Vec::new();
            if self.columns[name].serialize_meta(&mut stats_buf).is_ok() {
                out.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                out.extend_from_slice(&stats_buf);
            } else {
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }
        out
    }

    pub fn decode(data: &[u8], cursor: &mut usize) -> StorageResult<Self> {
        let take = |data: &[u8], cursor: &mut usize, len: usize| -> StorageResult<Vec<u8>> {
            if data.len().saturating_sub(*cursor) < len {
                return Err(StorageError::deserialize_error(
                    "segment stats record too short",
                ));
            }
            let slice = data[*cursor..*cursor + len].to_vec();
            *cursor += len;
            Ok(slice)
        };
        let version = u32::from_le_bytes(
            take(data, cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment stats version too short"))?,
        );
        if version != SEGMENT_STATS_RECORD_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported segment stats version: {}",
                version
            )));
        }
        let group = u32::from_le_bytes(
            take(data, cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment group too short"))?,
        );
        let row_count = u64::from_le_bytes(
            take(data, cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment row count too short"))?,
        );
        let live_count = u64::from_le_bytes(
            take(data, cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment live count too short"))?,
        );
        let null_count = u64::from_le_bytes(
            take(data, cursor, 8)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment null count too short"))?,
        );
        let has_min = take(data, cursor, 1)?[0] != 0;
        let sort_min = if has_min {
            let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().map_err(|_| {
                StorageError::deserialize_error("segment sort min length too short")
            })?) as usize;
            let bytes = take(data, cursor, len)?;
            let mut slice = &bytes[..];
            Some(deserialize_stat_value(&mut slice)?)
        } else {
            None
        };
        let has_max = take(data, cursor, 1)?[0] != 0;
        let sort_max = if has_max {
            let len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().map_err(|_| {
                StorageError::deserialize_error("segment sort max length too short")
            })?) as usize;
            let bytes = take(data, cursor, len)?;
            let mut slice = &bytes[..];
            Some(deserialize_stat_value(&mut slice)?)
        } else {
            None
        };
        let column_count = u32::from_le_bytes(
            take(data, cursor, 4)?
                .try_into()
                .map_err(|_| StorageError::deserialize_error("segment column count too short"))?,
        ) as usize;
        let mut columns = HashMap::with_capacity(column_count);
        for _ in 0..column_count {
            let name_len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().map_err(|_| {
                StorageError::deserialize_error("segment column name length too short")
            })?) as usize;
            let name_bytes = take(data, cursor, name_len)?;
            let name = String::from_utf8(name_bytes)
                .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
            let stats_len = u32::from_le_bytes(take(data, cursor, 4)?.try_into().map_err(|_| {
                StorageError::deserialize_error("segment column stats length too short")
            })?) as usize;
            let stats_bytes = take(data, cursor, stats_len)?;
            let mut slice = &stats_bytes[..];
            let stats = crate::column_stats::ColumnStats::deserialize_meta(&mut slice)?;
            if !slice.is_empty() {
                return Err(StorageError::deserialize_error(
                    "unexpected trailing data in segment column stats",
                ));
            }
            columns.insert(name, stats);
        }
        Ok(Self {
            group,
            row_count,
            live_count,
            null_count,
            sort_min,
            sort_max,
            columns,
        })
    }
}

/// Encode a full segment-statistics snapshot for one table checkpoint.
pub fn encode_segment_snapshot(stats: &HashMap<u32, GroupSegmentStats>) -> Vec<u8> {
    let mut groups: Vec<u32> = stats.keys().copied().collect();
    groups.sort_unstable();
    let mut out = Vec::new();
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for group in groups {
        out.extend_from_slice(&stats[&group].encode());
    }
    out
}

/// Decode a full segment-statistics snapshot, failing closed on version or
/// trailing mismatches.
pub fn decode_segment_snapshot(data: &[u8]) -> StorageResult<HashMap<u32, GroupSegmentStats>> {
    let mut cursor = 0usize;
    let take = |data: &[u8], cursor: &mut usize, len: usize| -> StorageResult<Vec<u8>> {
        if data.len().saturating_sub(*cursor) < len {
            return Err(StorageError::deserialize_error(
                "segment snapshot too short",
            ));
        }
        let slice = data[*cursor..*cursor + len].to_vec();
        *cursor += len;
        Ok(slice)
    };
    let version = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("segment snapshot version too short"))?,
    );
    if version != 1 {
        return Err(StorageError::deserialize_error(format!(
            "unsupported segment snapshot version: {}",
            version
        )));
    }
    let count = u32::from_le_bytes(
        take(data, &mut cursor, 4)?
            .try_into()
            .map_err(|_| StorageError::deserialize_error("segment snapshot count too short"))?,
    ) as usize;
    let mut out = HashMap::with_capacity(count);
    for _ in 0..count {
        let record = GroupSegmentStats::decode(data, &mut cursor)?;
        out.insert(record.group, record);
    }
    if cursor != data.len() {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in segment snapshot",
        ));
    }
    Ok(out)
}

/// Observable prune report for one filtered scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanPruneReport {
    pub segments_total: usize,
    pub segments_pruned: usize,
    pub rows_scanned: usize,
    pub rows_filtered: usize,
}

impl ScanPruneReport {
    pub fn prune_rate(&self) -> f64 {
        if self.segments_total == 0 {
            0.0
        } else {
            self.segments_pruned as f64 / self.segments_total as f64
        }
    }

    pub fn filter_rate(&self) -> f64 {
        if self.rows_scanned == 0 {
            0.0
        } else {
            self.rows_filtered as f64 / self.rows_scanned as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{EdgeSchema, EdgeStrategy};
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::edge_table::core::EdgeStore;
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;

    fn create_edge_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    fn create_edge_table_with_props() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_deletion_ratio() {
        let mut stats = DeletionStats::default();

        assert_eq!(stats.deletion_ratio(), 0.0);
        assert_eq!(stats.deletion_percentage(), 0.0);
        assert!(!stats.is_significant());

        stats.total_live_edges = 100;
        stats.total_deleted_edges = 50;
        assert!((stats.deletion_ratio() - 50.0 / 150.0).abs() < 1e-9);
        assert!((stats.deletion_percentage() - 100.0 * 50.0 / 150.0).abs() < 1e-9);
        assert!(stats.is_significant());

        stats.total_deleted_edges = 5;
        assert!((stats.deletion_ratio() - 5.0 / 105.0).abs() < 1e-9);
        assert!(!stats.is_significant());

        // Fully deleted tables saturate at 1.0 instead of overflowing.
        stats.total_live_edges = 0;
        stats.total_deleted_edges = 3;
        assert_eq!(stats.deletion_ratio(), 1.0);
    }

    #[test]
    fn test_tombstone_stats_accuracy() {
        let mut table = create_edge_table_with_props();

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 50)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 150)
            .unwrap();

        table.delete_edge(0, 1, 0, 200).unwrap();
        table.delete_edge(0, 2, 0, 250).unwrap();
        table.delete_edge(0, 3, 0, 300).unwrap();

        let stats = table.mvcc.tombstone_stats();
        // 3 deletions recorded once each in the authoritative table.
        assert_eq!(stats.count, 3);
        assert!(stats.memory_bytes > 0);
        assert_eq!(stats.oldest_delete_ts, Some(200));
        assert_eq!(stats.newest_delete_ts, Some(300));
    }

    #[test]
    fn test_deletion_stats_tracking() {
        let mut table = create_edge_table_with_props();

        for i in 0..5u64 {
            table
                .insert_edge(
                    0,
                    1,
                    i as i64,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100 + i,
                )
                .unwrap();
        }

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 0);
        assert_eq!(stats.deletion_percentage(), 0.0);

        table.delete_edge(0, 1, 0, 110).unwrap();
        table.delete_edge(0, 1, 1, 111).unwrap();

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 2);
        assert!(stats.deletion_percentage() >= 0.0);
    }

    #[test]
    fn test_deletion_stats_complete_table_deletion() {
        let mut table = create_edge_table();

        for i in 0..3 {
            table.insert_edge(0, 1, i as i64, &[], 100).unwrap();
        }

        for i in 0..3 {
            table.delete_edge(0, 1, i as i64, 110).unwrap();
        }

        let stats = table.deletion_stats();
        assert_eq!(stats.total_deleted_edges, 3);
        assert_eq!(stats.total_live_edges, 0);
    }

    #[test]
    fn test_segment_stats_collect_prune_and_roundtrip() {
        use crate::cursor::ScanPredicate;
        use std::collections::HashMap;

        let mut column_values: HashMap<String, Vec<Option<Value>>> = HashMap::new();
        column_values.insert(
            "weight".to_string(),
            vec![
                Some(Value::Double(1.0)),
                Some(Value::Double(2.0)),
                None,
                Some(Value::Double(9.0)),
            ],
        );
        let stats =
            GroupSegmentStats::collect(0, 4096, 3, &[10, 20, 30], &column_values, &HashMap::new());
        assert_eq!(stats.group, 0);
        assert_eq!(stats.row_count, 4096);
        assert_eq!(stats.live_count, 3);
        assert_eq!(stats.null_count, 1);
        assert_eq!(stats.sort_min, Some(Value::BigInt(10)));
        assert_eq!(stats.sort_max, Some(Value::BigInt(30)));

        let inside = vec![ScanPredicate::ColumnEqual {
            column: "weight".to_string(),
            value: Value::Double(2.0),
        }];
        assert!(stats.may_contain(&inside));
        let outside = vec![ScanPredicate::ColumnRange {
            column: "weight".to_string(),
            lower: Some(Value::Double(100.0)),
            upper: None,
            include_lower: true,
            include_upper: true,
        }];
        assert!(!stats.may_contain(&outside));
        let unknown = vec![ScanPredicate::ColumnEqual {
            column: "missing".to_string(),
            value: Value::Double(1.0),
        }];
        assert!(stats.may_contain(&unknown));

        let mut cursor = 0usize;
        let encoded = stats.encode();
        let decoded = GroupSegmentStats::decode(&encoded, &mut cursor).unwrap();
        assert_eq!(cursor, encoded.len());
        assert_eq!(decoded.group, stats.group);
        assert_eq!(decoded.row_count, stats.row_count);
        assert_eq!(decoded.live_count, stats.live_count);
        assert_eq!(decoded.null_count, stats.null_count);
        assert_eq!(decoded.sort_min, stats.sort_min);
        assert_eq!(decoded.sort_max, stats.sort_max);

        let mut snapshot = HashMap::new();
        snapshot.insert(0u32, stats);
        let bytes = encode_segment_snapshot(&snapshot);
        let restored = decode_segment_snapshot(&bytes).unwrap();
        assert_eq!(restored.len(), 1);
        assert!(restored.contains_key(&0));

        let mut bad = bytes.clone();
        bad.push(0);
        assert!(decode_segment_snapshot(&bad).is_err());
    }

    #[test]
    fn test_segment_stats_widen_only_bounds() {
        use std::collections::HashMap;

        let mut first_values: HashMap<String, Vec<Option<Value>>> = HashMap::new();
        first_values.insert(
            "weight".to_string(),
            vec![Some(Value::Double(5.0)), Some(Value::Double(6.0))],
        );
        let mut stats =
            GroupSegmentStats::collect(0, 4096, 2, &[5], &first_values, &HashMap::new());
        let mut second_values: HashMap<String, Vec<Option<Value>>> = HashMap::new();
        second_values.insert("weight".to_string(), vec![Some(Value::Double(1.0))]);
        let fresh = GroupSegmentStats::collect(1, 4096, 1, &[50], &second_values, &HashMap::new());
        stats.widen_with(&fresh);
        assert_eq!(stats.row_count, 4096);
        assert_eq!(stats.live_count, 1);
        assert_eq!(stats.sort_min, Some(Value::BigInt(5)));
        assert_eq!(stats.sort_max, Some(Value::BigInt(50)));
        let column = stats.columns.get("weight").unwrap();
        assert_eq!(column.min_value, Some(Value::Double(1.0)));
        assert_eq!(column.max_value, Some(Value::Double(6.0)));
    }

    #[test]
    fn test_prune_report_rates_are_observable() {
        let report = ScanPruneReport {
            segments_total: 4,
            segments_pruned: 1,
            rows_scanned: 100,
            rows_filtered: 25,
        };
        assert!((report.prune_rate() - 0.25).abs() < 1e-9);
        assert!((report.filter_rate() - 0.25).abs() < 1e-9);
        assert_eq!(ScanPruneReport::default().prune_rate(), 0.0);
        assert_eq!(ScanPruneReport::default().filter_rate(), 0.0);
    }
}

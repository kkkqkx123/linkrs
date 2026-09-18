//! Best-effort secondary property index maintenance.

use super::EdgeStore;
use crate::cursor::ScanPredicate;
use crate::index::edge_index_manager::EdgePropertyIndex;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageResult, Value};
use std::collections::HashSet;

use super::super::iterator::EdgeTableScanIterator;

impl EdgeStore {
    /// Secondary index write failures since the last rebuild or reset.
    pub fn index_failure_count(&self) -> u64 {
        self.index_write_failures
    }

    /// Reset the secondary index failure counter, typically after a rebuild.
    pub fn reset_index_failures(&mut self) {
        self.index_write_failures = 0;
    }

    /// Rebuild the property index when failures cross `threshold`.
    ///
    /// Returns true when a rebuild ran. Threshold policy lives with the
    /// operator; the store only guarantees the counter is monotonic between
    /// resets. A rebuild scans all live edges, so the fresh index resets the
    /// lag baseline to the build's own failure count.
    pub fn rebuild_property_index_on_failures(
        &mut self,
        threshold: u64,
        pool_capacity: u64,
    ) -> StorageResult<bool> {
        if self.property_index.is_none() || self.index_write_failures < threshold {
            return Ok(false);
        }
        self.build_property_index(pool_capacity)?;
        Ok(true)
    }

    fn record_index_write_failure(&mut self, prop_name: &str, latency_ms: u64) {
        self.index_write_failures = self.index_write_failures.saturating_add(1);
        if let Some(stats) = &self.stats_manager {
            stats.record_index_operation(self.label as u64, prop_name, latency_ms, false);
        }
    }

    fn record_index_write_success(&self, prop_name: &str, latency_ms: u64) {
        if let Some(stats) = &self.stats_manager {
            stats.record_index_operation(self.label as u64, prop_name, latency_ms, true);
        }
    }

    /// Fold one secondary index write outcome into the lag counter and the
    /// shared metrics registry. Primary data stays authoritative regardless.
    pub(crate) fn note_index_result(
        &mut self,
        prop_name: &str,
        result: StorageResult<()>,
        latency_ms: u64,
    ) {
        match result {
            Ok(()) => self.record_index_write_success(prop_name, latency_ms),
            Err(_) => self.record_index_write_failure(prop_name, latency_ms),
        }
    }

    pub(crate) fn update_property_index_on_delete(
        &mut self,
        properties: &Option<Vec<(String, Value)>>,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) {
        let Some(ref props) = properties else {
            return;
        };
        if self.property_index.is_none() {
            return;
        }
        let outcomes: Vec<(String, StorageResult<()>, u64)> =
            if let Some(ref mut index) = self.property_index {
                props
                    .iter()
                    .map(|(prop_name, prop_value)| {
                        let started = std::time::Instant::now();
                        let result = index.delete(prop_name, prop_value, src, dst, rank, ts);
                        let latency = started.elapsed().as_millis() as u64;
                        (prop_name.clone(), result, latency)
                    })
                    .collect()
            } else {
                Vec::new()
            };
        for (prop_name, result, latency) in outcomes {
            self.note_index_result(&prop_name, result, latency);
        }
    }

    /// Candidate edges from the secondary property index for one filter.
    ///
    /// Serves only every-equality conjunctions whose columns all carry an
    /// index, and only while the index carries no write lag: the index is
    /// best-effort, so any recorded failure falls back to the segment path
    /// instead of risking dropped hits. Stale entries resolve through the
    /// visibility authority, and the caller verifies every candidate back
    /// against the property columns.
    pub(crate) fn index_candidate_edge_ids(
        &self,
        predicates: &[ScanPredicate],
        query_ts: Timestamp,
    ) -> Option<Vec<EdgeId>> {
        let index = self.property_index.as_ref()?;
        if self.index_write_failures > 0 {
            return None;
        }
        let mut merged: Option<HashSet<EdgeId>> = None;
        for predicate in predicates {
            let (column, value) = match predicate {
                ScanPredicate::ColumnEqual { column, value } => (column, value),
                ScanPredicate::ColumnRange { .. } => return None,
            };
            if !index.has_index(column) {
                return None;
            }
            let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
            let (lower, upper) = codec.prefix_bounds(value).ok()?;
            let mut hits = HashSet::new();
            for ((src, dst, rank), _) in index.lookup(column, &lower, &upper) {
                if let Some(edge_id) = self.edge_id_of(src, dst, rank, query_ts) {
                    hits.insert(edge_id);
                }
            }
            merged = Some(match merged {
                None => hits,
                Some(prev) => prev.intersection(&hits).copied().collect(),
            });
            if merged.as_ref().is_some_and(HashSet::is_empty) {
                break;
            }
        }
        merged.map(|set| set.into_iter().collect())
    }

    /// Enable property index with the specified pool capacity.
    /// Builds the index from existing edge data.
    pub fn enable_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        self.build_property_index(pool_capacity)
    }

    /// Build the property index by scanning all edges.
    /// Streams one record at a time so peak memory stays flat instead of
    /// materializing every live edge plus decoded properties at once.
    /// The fresh scan resets the lag baseline: the failure counter ends at
    /// the build's own failure count, never silently cleared to zero.
    pub(crate) fn build_property_index(&mut self, pool_capacity: u64) -> StorageResult<()> {
        let mut index = EdgePropertyIndex::new(pool_capacity);
        // MAX_TIMESTAMP satisfies `create_ts <= ts < delete_ts` for live
        // edges, so all non-tombstoned edges are scanned.
        let all_ts = graphdb_core::types::MAX_TIMESTAMP;
        let label = self.label;

        let iter = EdgeTableScanIterator::new(self, all_ts);
        let mut build_failures: u64 = 0;
        let space_id = self.label as u64;
        let stats_manager = self.stats_manager.clone();
        for edge in iter {
            let src_u32 = edge.src_vid.as_int64().unwrap_or(0) as u32;
            let dst_u32 = edge.dst_vid.as_int64().unwrap_or(0) as u32;
            for (prop_name, prop_value) in &edge.properties {
                let started = std::time::Instant::now();
                let result = index.insert(
                    prop_name, prop_value, src_u32, dst_u32, edge.rank, label, all_ts,
                );
                let latency = started.elapsed().as_millis() as u64;
                if result.is_err() {
                    build_failures = build_failures.saturating_add(1);
                    if let Some(stats) = &stats_manager {
                        stats.record_index_operation(space_id, prop_name, latency, false);
                    }
                } else if let Some(stats) = &stats_manager {
                    stats.record_index_operation(space_id, prop_name, latency, true);
                }
            }
        }

        self.property_index = Some(index);
        self.index_write_failures = build_failures;
        if build_failures > 0 {
            log::debug!(
                "build_property_index: {} secondary writes failed, lag counter carries them",
                build_failures
            );
        }
        Ok(())
    }

    /// Check if property index is enabled.
    pub fn has_property_index(&self) -> bool {
        self.property_index.is_some()
    }

    /// Drop the property index to free memory.
    pub fn disable_property_index(&mut self) {
        self.property_index = None;
    }

    /// Lookup edges by a property value range using the EdgePropertyIndex.
    /// Returns `(src, dst, rank)` tuples for matching edges.
    pub fn lookup_edges_by_property_range(
        &self,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
    ) -> Vec<(u32, u32, i64)> {
        let Some(ref index) = self.property_index else {
            return Vec::new();
        };
        if !index.has_index(prop_name) {
            return Vec::new();
        }
        index
            .lookup(prop_name, value_lower, value_upper)
            .into_iter()
            .map(|((src, dst, rank), _record)| (src, dst, rank))
            .collect()
    }
}

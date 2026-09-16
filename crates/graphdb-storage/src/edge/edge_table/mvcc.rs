//! MVCC and tombstone management: snapshot isolation and garbage collection.
//!
//! Visibility authority is `edge_timestamps` alone. The tombstone table is a
//! derived index for garbage-collection enumeration, maintained through the
//! record/remove entry points; it never overrules the authority. CSR row
//! stamps and adjacency `Nbr` stamps are physical projections for collection
//! only. Every visibility decision goes through
//! [`MVCCManager::is_edge_visible`] (or its pending-aware overload) so the
//! copies cannot drift apart.

use super::stats::TombstoneStats;
use graphdb_core::types::{EdgeId, Timestamp};
use std::collections::HashMap;

/// Per-edge creation and deletion timestamps.
#[derive(Debug, Clone, Copy)]
pub struct EdgeTimestamps {
    pub create_ts: Timestamp,
    pub delete_ts: Timestamp,
}

impl EdgeTimestamps {
    pub fn new(create_ts: Timestamp) -> Self {
        Self {
            create_ts,
            delete_ts: Timestamp::MAX,
        }
    }

    pub fn is_alive_at(&self, ts: Timestamp) -> bool {
        crate::mvcc_visibility::Visibility::is_edge_visible(ts, self.create_ts, self.delete_ts)
    }
}

/// MVCC and snapshot management for the single-segment edge table.
///
/// Single authority for edge visibility. `active_snapshots` /
/// `min_active_snapshot_ts` are a per-table pin cache; the GC truth source
/// is the transaction layer (`SnapshotTracker`, unified per pass via
/// `MvccWatermarks`), so pass-level cutoffs must come from captured
/// watermarks, never from this cache alone.
pub struct MVCCManager {
    /// Per-edge creation/deletion timestamps (visibility authority).
    pub edge_timestamps: HashMap<EdgeId, EdgeTimestamps>,
    /// Tombstone table: edge id to earliest `delete_ts`.
    pub tombstones: HashMap<EdgeId, Timestamp>,
    /// Minimum timestamp of all active snapshots.
    pub min_active_snapshot_ts: Timestamp,
    /// Active snapshot timestamps and their reference count.
    pub active_snapshots: HashMap<Timestamp, usize>,
}

impl Default for MVCCManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MVCCManager {
    /// Create a new MVCC manager
    pub fn new() -> Self {
        Self {
            edge_timestamps: HashMap::new(),
            tombstones: HashMap::new(),
            min_active_snapshot_ts: Timestamp::MAX,
            active_snapshots: HashMap::new(),
        }
    }

    /// Check if an edge is tombstoned at a given timestamp.
    pub fn is_tombstoned(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        self.tombstones
            .get(&edge_id)
            .is_some_and(|delete_ts| *delete_ts <= ts)
    }

    /// Drop tombstones eligible under `Visibility::is_gc_eligible`. Safe:
    /// `is_edge_visible` consults `edge_timestamps.delete_ts` first, which
    /// keeps hiding the edge from every snapshot at or past deletion.
    pub fn gc_tombstones(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        self.gc_tombstones_batch(min_active_snapshot_ts, usize::MAX)
    }

    /// Inspect at most `batch_size` tombstones.
    pub fn gc_tombstones_batch(
        &mut self,
        min_active_snapshot_ts: Timestamp,
        batch_size: usize,
    ) -> usize {
        if batch_size == 0 {
            return 0;
        }
        let mut eligible: Vec<EdgeId> = self
            .tombstones
            .iter()
            .filter_map(|(edge_id, delete_ts)| {
                crate::mvcc_visibility::Visibility::is_gc_eligible(
                    *delete_ts,
                    min_active_snapshot_ts,
                )
                .then_some(*edge_id)
            })
            .collect();
        eligible.truncate(batch_size);
        let mut removed = 0;
        for edge_id in eligible {
            if self.tombstones.remove(&edge_id).is_some() {
                removed += 1;
            }
        }
        removed
    }

    /// Register a new active snapshot at the given timestamp.
    ///
    /// This increments the reference count for the snapshot timestamp.
    /// Uses incremental min maintenance to avoid O(n) scans.
    pub fn register_active_snapshot(&mut self, ts: Timestamp) {
        *self.active_snapshots.entry(ts).or_insert(0) += 1;
        // Incremental min maintenance: a new snapshot can only lower the
        // minimum, so compare against the current value instead of rescanning
        // the whole map.
        if ts < self.min_active_snapshot_ts {
            self.min_active_snapshot_ts = ts;
        }
    }

    /// Unregister an active snapshot at the given timestamp.
    ///
    /// This decrements the reference count. When count reaches 0 the
    /// timestamp is removed and the cached minimum is recomputed.
    ///
    /// Deliberately performs no garbage collection: the table-local minimum
    /// only sees this table's readers, while tombstones may still pin
    /// readers of other tables. Reclamation always derives its cutoff from
    /// the global watermarks (`MvccWatermarks::capture`) at pass level —
    /// see `gc_tombstones` callers — never from this cache alone.
    pub fn unregister_active_snapshot(&mut self, ts: Timestamp) -> usize {
        let mut removed_min = false;
        let new_count = if let Some(count) = self.active_snapshots.get_mut(&ts) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                self.active_snapshots.remove(&ts);
                removed_min = ts == self.min_active_snapshot_ts;
                0
            } else {
                *count
            }
        } else {
            0
        };

        // Only rescan when the removed timestamp was the current minimum;
        // otherwise the min is unchanged. No GC here by design (see docs).
        if removed_min {
            self.min_active_snapshot_ts = self
                .active_snapshots
                .keys()
                .copied()
                .min()
                .unwrap_or(Timestamp::MAX);
        }

        new_count
    }

    /// Get current tombstone statistics for observability.
    pub fn tombstone_stats(&self) -> TombstoneStats {
        let count = self.tombstones.len();
        let oldest = self.tombstones.values().copied().min();
        let newest = self.tombstones.values().copied().max();
        TombstoneStats {
            count,
            memory_bytes: TombstoneStats::estimate_memory(count),
            oldest_delete_ts: oldest,
            newest_delete_ts: newest,
        }
    }

    /// Total count of deletions (for memory accounting).
    pub fn total_tombstone_count(&self) -> usize {
        self.tombstones.len()
    }

    /// Record a deletion in the authoritative tombstone table.
    ///
    /// Single entry point for every deletion path. Keeps the earliest
    /// `delete_ts` when the same edge is recorded more than once: an
    /// earlier deletion covers a wider query range and must win.
    pub fn record_deletion(&mut self, edge_id: EdgeId, delete_ts: Timestamp) {
        let earliest = self
            .tombstones
            .get(&edge_id)
            .map_or(delete_ts, |ts| (*ts).min(delete_ts));
        self.tombstones.insert(edge_id, earliest);
    }

    /// Undo of [`Self::record_deletion`]: remove the tombstone for `edge_id`.
    ///
    /// Returns true when a tombstone was present and removed.
    pub fn remove_deletion(&mut self, edge_id: EdgeId) -> bool {
        self.tombstones.remove(&edge_id).is_some()
    }

    /// Get number of active snapshots (for testing and debugging)
    #[cfg(test)]
    pub fn active_snapshot_count(&self) -> usize {
        self.active_snapshots.values().sum()
    }

    // ── Per-edge timestamp management (centralized MVCC) ──

    /// Record edge creation. Called on insert_edge to register the edge's
    /// creation timestamp in the centralized MVCC store.
    pub fn record_creation(&mut self, edge_id: EdgeId, create_ts: Timestamp) {
        self.edge_timestamps
            .insert(edge_id, EdgeTimestamps::new(create_ts));
    }

    /// Record edge deletion (logical). Called on delete_edge to set the
    /// edge's deletion timestamp and record the tombstone.
    pub fn record_edge_deletion(&mut self, edge_id: EdgeId, delete_ts: Timestamp) {
        if let Some(ts) = self.edge_timestamps.get_mut(&edge_id) {
            ts.delete_ts = ts.delete_ts.min(delete_ts);
        }
        self.record_deletion(edge_id, delete_ts);
    }

    /// Check if an edge is visible at a given timestamp.
    ///
    /// This is the single entry point for all MVCC visibility decisions.
    /// When the authority record exists it decides alone; the tombstone
    /// table is consulted only for edges without an authority record.
    pub fn is_edge_visible(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        if let Some(ts_info) = self.edge_timestamps.get(&edge_id) {
            return crate::mvcc_visibility::Visibility::is_edge_visible(
                ts,
                ts_info.create_ts,
                ts_info.delete_ts,
            );
        }
        !self.is_tombstoned(edge_id, ts)
    }

    /// Pending-aware overload of [`Self::is_edge_visible`].
    ///
    /// Same single authority, but creation/deletion stamps owned by foreign
    /// uncommitted transactions are filtered through `gate`: a foreign
    /// pending creation hides the edge, a foreign pending deletion in the
    /// authority stamps is ignored. Edges without an authority record fall
    /// back to the tombstone table. Operation-layer scans funnel through
    /// the table `*_with_gate` methods.
    pub fn is_edge_visible_with_gate(
        &self,
        edge_id: EdgeId,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> bool {
        if let Some(ts_info) = self.edge_timestamps.get(&edge_id) {
            if ts_info.delete_ts != Timestamp::MAX
                && ts_info.delete_ts <= ts
                && gate.is_foreign_pending(ts, ts_info.delete_ts)
            {
                return true;
            }
            return gate.is_edge_visible(ts, ts_info.create_ts, ts_info.delete_ts);
        }
        if let Some(delete_ts) = self.tombstones.get(&edge_id) {
            if *delete_ts <= ts && gate.is_foreign_pending(ts, *delete_ts) {
                return true;
            }
        }
        !self.is_tombstoned(edge_id, ts)
    }

    /// Get the creation timestamp of an edge, if known.
    pub fn creation_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        self.edge_timestamps.get(&edge_id).map(|ts| ts.create_ts)
    }

    /// Get the deletion timestamp of an edge, if deleted.
    pub fn deletion_ts_of(&self, edge_id: EdgeId) -> Option<Timestamp> {
        self.edge_timestamps
            .get(&edge_id)
            .filter(|ts| ts.delete_ts != Timestamp::MAX)
            .map(|ts| ts.delete_ts)
    }

    /// Check if an edge has been deleted (tombstoned or has delete_ts < MAX).
    pub fn is_edge_deleted(&self, edge_id: EdgeId) -> bool {
        if let Some(ts) = self.edge_timestamps.get(&edge_id) {
            return ts.delete_ts != Timestamp::MAX;
        }
        self.tombstones.contains_key(&edge_id)
    }

    /// Remove edge timestamps. Called during rollback of a failed insert.
    pub fn remove_edge_timestamps(&mut self, edge_id: EdgeId) {
        self.edge_timestamps.remove(&edge_id);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::edge_table::core::EdgeStore;
    use graphdb_core::types::EdgeId;
    use graphdb_core::Value;

    fn create_edge_table_with_props() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::types::StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_gc_tombstones_basic() {
        let mut table = create_edge_table_with_props();

        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        table.insert_edge(0, 2, 0, &[], 100).unwrap();
        table.insert_edge(0, 3, 0, &[], 100).unwrap();

        table.mvcc.tombstones.insert(EdgeId(0), 200);
        table.mvcc.tombstones.insert(EdgeId(1), 250);
        table.mvcc.tombstones.insert(EdgeId(2), 300);

        assert_eq!(table.mvcc.tombstones.len(), 3);

        let removed = table.mvcc.gc_tombstones(220);
        assert_eq!(removed, 1);
        assert_eq!(table.mvcc.tombstones.len(), 2);

        let removed = table.mvcc.gc_tombstones(260);
        assert_eq!(removed, 1);
        assert_eq!(table.mvcc.tombstones.len(), 1);

        let removed = table.mvcc.gc_tombstones(310);
        assert_eq!(removed, 1);
        assert_eq!(table.mvcc.tombstones.len(), 0);
    }

    #[test]
    fn test_gc_tombstones_boundary_preserves_visibility() {
        let mut table = create_edge_table_with_props();

        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        let edge_id = EdgeId(0);
        table.mvcc.record_edge_deletion(edge_id, 200);

        assert!(table.mvcc.is_edge_visible(edge_id, 199));
        assert!(!table.mvcc.is_edge_visible(edge_id, 200));
        assert!(!table.mvcc.is_edge_visible(edge_id, 201));

        // The exclusive waterfront reclaims `delete_ts == safe`: the
        // redundant tombstone entry goes away...
        let removed = table.mvcc.gc_tombstones(200);
        assert_eq!(removed, 1);

        // ...while every visibility decision stays identical because
        // `edge_timestamps.delete_ts` remains authoritative.
        assert!(table.mvcc.is_edge_visible(edge_id, 199));
        assert!(!table.mvcc.is_edge_visible(edge_id, 200));
        assert!(!table.mvcc.is_edge_visible(edge_id, 201));
    }

    #[test]
    fn test_gc_tombstones_preserves_active_snapshots() {
        let mut table = create_edge_table_with_props();

        table.mvcc.tombstones.insert(EdgeId(0), 200);
        assert_eq!(table.mvcc.tombstones.len(), 1);

        let removed = table.mvcc.gc_tombstones(151);
        assert_eq!(removed, 0);
        assert_eq!(table.mvcc.tombstones.len(), 1);

        let removed = table.mvcc.gc_tombstones(201);
        assert_eq!(removed, 1);
        assert_eq!(table.mvcc.tombstones.len(), 0);
    }

    #[test]
    fn test_gc_tombstones_boundary_uses_exclusive_waterfront() {
        let mut manager = MVCCManager::new();
        manager.record_creation(EdgeId(0), 100);
        manager.record_creation(EdgeId(1), 100);
        manager.record_edge_deletion(EdgeId(0), 200);
        manager.record_edge_deletion(EdgeId(1), 201);

        // The exclusive waterfront reclaims `delete_ts == safe`; the
        // redundant tombstone entry goes away one round earlier.
        let removed = manager.gc_tombstones(200);
        assert_eq!(removed, 1);
        assert_eq!(manager.tombstones.len(), 1);

        // Deletions stay observable through `edge_timestamps` regardless.
        assert!(!manager.is_edge_visible(EdgeId(0), 200));
        assert!(!manager.is_edge_visible(EdgeId(1), 201));

        let removed = manager.gc_tombstones(201);
        assert_eq!(removed, 1);
        assert_eq!(manager.tombstones.len(), 0);
    }

    #[test]
    fn test_incremental_gc_never_exceeds_batch() {
        let mut manager = MVCCManager::new();
        for id in 0..100u64 {
            manager.tombstones.insert(EdgeId(id), 10);
        }

        let removed = manager.gc_tombstones_batch(20, 11);
        assert_eq!(removed, 11);
        assert_eq!(manager.tombstones.len(), 89);
    }

    #[test]
    fn test_tombstones_gc_multiple_edges() {
        let mut table = create_edge_table_with_props();

        for i in 0..10u64 {
            table.mvcc.tombstones.insert(EdgeId(i), 100 + (i * 10));
        }

        assert_eq!(table.mvcc.tombstones.len(), 10);

        let removed = table.mvcc.gc_tombstones(150);
        assert_eq!(removed, 6);
        assert_eq!(table.mvcc.tombstones.len(), 4);

        for &delete_ts in table.mvcc.tombstones.values() {
            assert!(delete_ts > 150);
        }
    }

    #[test]
    fn test_snapshot_lifecycle_never_gc_implicitly() {
        let mut table = create_edge_table_with_props();

        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();

        table.delete_edge(0, 1, 0, 150).unwrap();

        let stats_before = table.mvcc.tombstone_stats();
        assert_eq!(stats_before.count, 1);

        table.mvcc.register_active_snapshot(100);
        table.mvcc.register_active_snapshot(100);
        table.mvcc.register_active_snapshot(120);

        let count_after_first = table.mvcc.unregister_active_snapshot(100);
        assert_eq!(count_after_first, 1);

        // Unregistering snapshots is pure bookkeeping: tombstones survive
        // until an explicit watermark-driven pass reclaims them, so a
        // table-local minimum can never free another table's readers.
        let count_after_second = table.mvcc.unregister_active_snapshot(100);
        assert_eq!(count_after_second, 0);

        let count_120 = table.mvcc.unregister_active_snapshot(120);
        assert_eq!(count_120, 0);

        let stats_after_unregister = table.mvcc.tombstone_stats();
        assert_eq!(stats_after_unregister.count, 1);

        let removed = table.mvcc.gc_tombstones(Timestamp::MAX);
        assert_eq!(removed, 1);
        let stats_after_gc = table.mvcc.tombstone_stats();
        assert_eq!(stats_after_gc.count, 0);
    }

    #[test]
    fn test_visibility_uses_single_predicate() {
        let mut table = create_edge_table_with_props();
        table.insert_edge(0, 1, 0, &[], 100).unwrap();
        assert!(table.has_edge(0, 1, 0, 100));
        assert!(!table.has_edge(0, 1, 0, 99));
        assert!(table.delete_edge(0, 1, 0, 150).unwrap());
        assert!(table.has_edge(0, 1, 0, 149));
        assert!(!table.has_edge(0, 1, 0, 150));
    }

    #[test]
    fn test_mvcc_metrics_gc_count() {
        let mut table = create_edge_table_with_props();

        for i in 0..5u64 {
            table
                .insert_edge(
                    0,
                    1,
                    i as i64,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    i,
                )
                .unwrap();
        }

        table.delete_edge(0, 1, 0, 2).unwrap();
        table.delete_edge(0, 1, 1, 3).unwrap();

        table.mvcc.register_active_snapshot(1);
        table.mvcc.register_active_snapshot(4);

        assert_eq!(table.mvcc.total_tombstone_count(), 2);

        // GC removes entries from the single authoritative table: both edges
        // deleted at ts <= 3 are covered by the exclusive waterfront, while
        // their deletions stay observable through `edge_timestamps`.
        let removed = table.mvcc.gc_tombstones(3);
        assert_eq!(removed, 2);
        assert_eq!(table.mvcc.tombstones.len(), 0);
    }

    #[test]
    fn test_mvcc_metrics_tombstone_count() {
        use graphdb_metrics::{MetricType, StatsManager};
        use std::sync::Arc;

        let mut table = create_edge_table_with_props();

        let stats_manager = Arc::new(StatsManager::new());
        table.set_stats_manager(stats_manager.clone());

        for i in 0..5u64 {
            table
                .insert_edge(
                    0,
                    1,
                    i as i64,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    i,
                )
                .unwrap();
        }

        table.delete_edge(0, 1, 0, 10).unwrap();
        table.delete_edge(0, 1, 1, 11).unwrap();
        table.delete_edge(0, 1, 2, 12).unwrap();

        let tom_stats = table.mvcc.tombstone_stats();
        assert_eq!(tom_stats.count, 3);

        stats_manager.record_tombstone_stats(
            tom_stats.count as u64,
            tom_stats.memory_bytes as u64,
            tom_stats.oldest_delete_ts.map(|ts| ts as u32),
            tom_stats.newest_delete_ts.map(|ts| ts as u32),
            1,
        );

        let tombstone_count = stats_manager
            .get_value(MetricType::TombstoneCount)
            .unwrap_or(0);
        assert_eq!(tombstone_count, 3);

        let tombstone_memory = stats_manager
            .get_value(MetricType::TombstoneMemoryBytes)
            .unwrap_or(0);
        assert!(tombstone_memory > 0);
    }

    #[test]
    fn test_mvcc_metrics_active_snapshots() {
        let mut table = create_edge_table_with_props();

        table.mvcc.register_active_snapshot(1);
        assert_eq!(table.mvcc.active_snapshot_count(), 1);

        table.mvcc.register_active_snapshot(2);
        assert_eq!(table.mvcc.active_snapshot_count(), 2);

        table.mvcc.unregister_active_snapshot(1);
        assert_eq!(table.mvcc.active_snapshot_count(), 1);
    }

    #[test]
    fn test_record_deletion_keeps_earliest_ts() {
        let mut mvcc = MVCCManager::new();

        mvcc.record_deletion(EdgeId(7), 200);
        mvcc.record_deletion(EdgeId(7), 150);

        // The earlier deletion wins: it covers a wider query range.
        assert_eq!(mvcc.tombstones.get(&EdgeId(7)), Some(&150));
        assert!(mvcc.is_tombstoned(EdgeId(7), 200));
        assert!(!mvcc.is_tombstoned(EdgeId(7), 100));
    }

    #[test]
    fn test_record_deletion_deduplicates() {
        let mut mvcc = MVCCManager::new();

        // Repeated deletions of the same edge must not grow the tombstone
        // count: the authoritative table keeps a single entry with the
        // earliest delete_ts.
        mvcc.record_deletion(EdgeId(3), 150);
        mvcc.record_deletion(EdgeId(3), 200);

        assert_eq!(mvcc.tombstones.len(), 1);
        assert_eq!(mvcc.tombstones.get(&EdgeId(3)), Some(&150));
        assert_eq!(mvcc.total_tombstone_count(), 1);
        assert!(mvcc.is_tombstoned(EdgeId(3), 200));
        assert!(!mvcc.is_tombstoned(EdgeId(3), 100));
    }
}

//! MVCC and tombstone management: snapshot isolation and garbage collection.
//!
//! Provides multi-version concurrency control through active snapshot tracking,
//! tombstone lifecycle management, and automatic garbage collection.
//!
//! Tombstone management uses a tiered approach:
//! - Hot layer: recent deletions in main hashtable (fast path, LRU-like eviction)
//! - Cold layer: older deletions preserved for snapshot isolation
//!
//! This reduces lookup overhead and memory fragmentation for large deletion sets.

use std::collections::HashMap;
use super::stats::TombstoneStats;
use crate::core::types::{Timestamp, EdgeId};

const HOT_TOMBSTONE_MAX_SIZE: usize = 100_000;
const HOT_TOMBSTONE_GC_THRESHOLD: usize = 150_000;

/// MVCC and snapshot management for EdgeTable
pub struct MVCCManager {
    /// Deletions of edges still in mutable CSR or recently deleted (hot layer)
    pub pending_segment_deletions: HashMap<EdgeId, Timestamp>,
    /// Deletions of edges already in frozen segments (hot layer)
    pub segment_tombstones: HashMap<EdgeId, Timestamp>,
    /// Legacy tombstones field for backward compatibility during transition (hot layer)
    pub tombstones: HashMap<EdgeId, Timestamp>,
    /// Cold layer: older tombstones beyond hot threshold, kept for snapshot isolation
    /// Stored as Vec<(EdgeId, Timestamp)> to save memory and reduce lookup overhead
    pub cold_tombstones: Vec<(EdgeId, Timestamp)>,
    /// Minimum timestamp of all active snapshots
    pub min_active_snapshot_ts: Timestamp,
    /// Active snapshot timestamps and their reference count
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
            pending_segment_deletions: HashMap::new(),
            segment_tombstones: HashMap::new(),
            tombstones: HashMap::new(),
            cold_tombstones: Vec::new(),
            min_active_snapshot_ts: u32::MAX,
            active_snapshots: HashMap::new(),
        }
    }

    /// Check if an edge is tombstoned at a given timestamp
    /// Uses hot-first lookup: checks hashtables first, then cold layer with binary search
    pub fn is_tombstoned(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        // Hot layer: fast path - O(1) average
        let pending_deleted = self.pending_segment_deletions
            .get(&edge_id)
            .is_some_and(|delete_ts| *delete_ts <= ts);

        let segment_deleted = self.segment_tombstones
            .get(&edge_id)
            .is_some_and(|delete_ts| *delete_ts <= ts);

        let legacy_deleted = self.tombstones
            .get(&edge_id)
            .is_some_and(|delete_ts| *delete_ts <= ts);

        if pending_deleted || segment_deleted || legacy_deleted {
            return true;
        }

        // Cold layer: only checked if hot layer misses - O(log n) binary search
        self.is_tombstoned_cold(edge_id, ts)
    }

    /// Check if an edge is tombstoned in cold layer using binary search.
    ///
    /// The cold layer is kept sorted by EdgeId for efficient lookups.
    /// Returns true if the edge exists in cold layer with delete_ts <= ts.
    fn is_tombstoned_cold(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        match self.cold_tombstones.binary_search_by_key(&edge_id, |&(id, _)| id) {
            Ok(idx) => self.cold_tombstones[idx].1 <= ts,
            Err(_) => false,
        }
    }

    /// Garbage collect tombstones that are no longer needed for snapshot isolation.
    ///
    /// Removes tombstones with delete_ts < min_active_snapshot_ts.
    /// These tombstones cannot affect any active snapshot since all snapshots
    /// have ts >= min_active_snapshot_ts.
    ///
    /// Also manages hot/cold layer promotion: if hot layer exceeds threshold,
    /// older entries are moved to cold layer (kept sorted by EdgeId for binary search).
    pub fn gc_tombstones(&mut self, min_active_snapshot_ts: Timestamp) -> usize {
        let before = self.tombstones.len() + self.cold_tombstones.len();

        // Clean hot layer
        self.tombstones.retain(|_edge_id, delete_ts| {
            *delete_ts >= min_active_snapshot_ts
        });

        // Clean cold layer
        self.cold_tombstones.retain(|(_, delete_ts)| {
            *delete_ts >= min_active_snapshot_ts
        });

        self.min_active_snapshot_ts = min_active_snapshot_ts;

        // If hot layer is too large, move old entries to cold layer
        // Cold layer is kept sorted by EdgeId for efficient binary search
        if self.tombstones.len() > HOT_TOMBSTONE_GC_THRESHOLD {
            let mut to_move = Vec::new();
            for (edge_id, ts) in self.tombstones.iter() {
                to_move.push((*edge_id, *ts));
            }
            // Sort by EdgeId to maintain cold layer invariant for binary search
            to_move.sort_by_key(|k| k.0);

            let move_count = (to_move.len() as f64 * 0.3) as usize;
            for i in 0..move_count {
                let (edge_id, ts) = to_move[i];
                self.tombstones.remove(&edge_id);
                self.cold_tombstones.push((edge_id, ts));
            }

            // Ensure cold layer remains sorted for binary search
            self.cold_tombstones.sort_by_key(|k| k.0);
        }

        let after = self.tombstones.len() + self.cold_tombstones.len();
        before.saturating_sub(after)
    }

    /// Register a new active snapshot at the given timestamp.
    ///
    /// This increments the reference count for the snapshot timestamp.
    /// Must be called when a new snapshot is created.
    pub fn register_active_snapshot(&mut self, ts: Timestamp) {
        *self.active_snapshots.entry(ts).or_insert(0) += 1;
    }

    /// Unregister an active snapshot at the given timestamp.
    ///
    /// This decrements the reference count. When count reaches 0,
    /// the timestamp is removed and tombstone GC is automatically triggered.
    pub fn unregister_active_snapshot(&mut self, ts: Timestamp) -> usize {
        let mut should_gc = false;
        let new_count = if let Some(count) = self.active_snapshots.get_mut(&ts) {
            if *count > 0 {
                *count -= 1;
            }
            if *count == 0 {
                self.active_snapshots.remove(&ts);
                should_gc = true;
                0
            } else {
                *count
            }
        } else {
            0
        };

        if should_gc {
            let new_min_ts = self.active_snapshots
                .keys()
                .copied()
                .min()
                .unwrap_or(u32::MAX);
            self.gc_tombstones(new_min_ts);
        }

        new_count
    }

    /// Get current tombstone statistics for observability.
    pub fn tombstone_stats(&self) -> TombstoneStats {
        let hot_count = self.tombstones.len();
        let cold_count = self.cold_tombstones.len();
        let total_count = hot_count + cold_count;

        let oldest = self.tombstones
            .values()
            .chain(self.cold_tombstones.iter().map(|(_, ts)| ts))
            .copied()
            .min();

        let newest = self.tombstones
            .values()
            .chain(self.cold_tombstones.iter().map(|(_, ts)| ts))
            .copied()
            .max();

        TombstoneStats {
            count: total_count,
            memory_bytes: TombstoneStats::estimate_memory(hot_count) +
                (cold_count * std::mem::size_of::<(EdgeId, Timestamp)>()),
            oldest_delete_ts: oldest,
            newest_delete_ts: newest,
            min_active_snapshot_ts: self.min_active_snapshot_ts,
        }
    }

    /// Get the minimum active snapshot timestamp.
    ///
    /// This is the earliest timestamp at which any snapshot is currently active.
    /// All tombstones with delete_ts < this value can be safely garbage collected.
    pub fn get_min_active_snapshot_ts(&self) -> Timestamp {
        self.active_snapshots
            .keys()
            .copied()
            .min()
            .unwrap_or(u32::MAX)
    }

    /// Get number of active snapshots (for testing and debugging)
    #[cfg(test)]
    pub fn active_snapshot_count(&self) -> usize {
        self.active_snapshots.values().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use super::*;
    use crate::core::Value;
    use crate::core::types::EdgeId;

    fn create_edge_table_with_props() -> EdgeTable {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        };
        EdgeTable::new(schema).unwrap()
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
    fn test_tombstones_gc_multiple_edges() {
        let mut table = create_edge_table_with_props();

        for i in 0..10 {
            table.mvcc.tombstones.insert(EdgeId(i), 100 + (i as u32 * 10));
        }

        assert_eq!(table.mvcc.tombstones.len(), 10);

        let removed = table.mvcc.gc_tombstones(150);
        assert_eq!(removed, 5);
        assert_eq!(table.mvcc.tombstones.len(), 5);

        for &delete_ts in table.mvcc.tombstones.values() {
            assert!(delete_ts >= 150);
        }
    }

    #[test]
    fn test_auto_gc_with_snapshot_lifecycle() {
        let mut table = create_edge_table_with_props();

        table.insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100).unwrap();

        table.freeze_csr_only(125);

        table.delete_edge(0, 1, 0, 150).unwrap();

        let stats_before = table.mvcc.tombstone_stats();
        assert_eq!(stats_before.count, 1);

        table.mvcc.register_active_snapshot(100);
        table.mvcc.register_active_snapshot(100);
        table.mvcc.register_active_snapshot(120);

        let count_after_first = table.mvcc.unregister_active_snapshot(100);
        assert_eq!(count_after_first, 1);

        let stats_after_first = table.mvcc.tombstone_stats();
        assert_eq!(stats_after_first.count, 1);

        let count_after_second = table.mvcc.unregister_active_snapshot(100);
        assert_eq!(count_after_second, 0);

        let count_120 = table.mvcc.unregister_active_snapshot(120);
        assert_eq!(count_120, 0);

        let stats_after_gc = table.mvcc.tombstone_stats();
        assert_eq!(stats_after_gc.count, 0);
    }

    #[test]
    fn test_mvcc_metrics_gc_count() {
        let mut table = create_edge_table_with_props();

        for i in 0..5 {
            table.insert_edge(0, 1, i as i64, &[("weight".to_string(), Value::Double(i as f64))], i as u32).unwrap();
        }

        table.freeze_csr_only(5);

        table.delete_edge(0, 1, 0, 2).unwrap();
        table.delete_edge(0, 1, 1, 3).unwrap();

        table.mvcc.register_active_snapshot(1);
        table.mvcc.register_active_snapshot(4);

        assert_eq!(table.mvcc.tombstones.len(), 2);

        let removed = table.mvcc.gc_tombstones(3);
        assert_eq!(removed, 1);
        assert_eq!(table.mvcc.tombstones.len(), 1);
    }

    #[test]
    fn test_mvcc_metrics_tombstone_count() {
        use crate::core::stats::{MetricType, StatsManager};
        use std::sync::Arc;

        let mut table = create_edge_table_with_props();

        let stats_manager = Arc::new(StatsManager::new());
        table.set_stats_manager(stats_manager.clone());

        for i in 0..5 {
            table.insert_edge(0, 1, i as i64, &[("weight".to_string(), Value::Double(i as f64))], i as u32).unwrap();
        }

        table.freeze_csr_only(5);

        table.delete_edge(0, 1, 0, 10).unwrap();
        table.delete_edge(0, 1, 1, 11).unwrap();
        table.delete_edge(0, 1, 2, 12).unwrap();

        let tom_stats = table.mvcc.tombstone_stats();
        assert_eq!(tom_stats.count, 3);

        stats_manager.record_tombstone_stats(
            tom_stats.count as u64,
            tom_stats.memory_bytes as u64,
            tom_stats.oldest_delete_ts,
            tom_stats.newest_delete_ts,
            1,
        );

        let tombstone_count = stats_manager.get_value(MetricType::TombstoneCount).unwrap_or(0);
        assert_eq!(tombstone_count, 3);

        let tombstone_memory = stats_manager.get_value(MetricType::TombstoneMemoryBytes).unwrap_or(0);
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
    fn test_cold_layer_binary_search() {
        let mut mvcc = MVCCManager::new();

        // Add 100 tombstones to cold layer, sorted by EdgeId
        for i in 0..100 {
            mvcc.cold_tombstones.push((EdgeId(i as u64), 100 + i as u32));
        }
        mvcc.cold_tombstones.sort_by_key(|k| k.0);

        // Test binary search - all should be found
        for i in 0..100 {
            assert!(mvcc.is_tombstoned_cold(EdgeId(i as u64), u32::MAX));
        }

        // Test edge cases
        assert!(!mvcc.is_tombstoned_cold(EdgeId(200), u32::MAX)); // Not in cold layer
        assert!(!mvcc.is_tombstoned_cold(EdgeId(0), 50)); // Before delete_ts
    }

    #[test]
    fn test_cold_layer_lookup_performance() {
        let mut mvcc = MVCCManager::new();

        // Add 100K tombstones to cold layer, simulating large delete set
        for i in 0..100_000 {
            mvcc.cold_tombstones.push((EdgeId(i as u64), u32::MAX - 1));
        }
        mvcc.cold_tombstones.sort_by_key(|k| k.0);

        let start = std::time::Instant::now();
        for i in 0..10_000 {
            let idx = i * 10; // Sparse queries
            let _ = mvcc.is_tombstoned(EdgeId(idx as u64), u32::MAX);
        }
        let elapsed = start.elapsed();

        // O(log n) should complete in microseconds for 10K queries over 100K items
        // This should be well under 100ms (typical desktop: ~10-30ms for optimized binary search)
        println!("Cold layer lookup performance: 10K queries over 100K items in {:?}", elapsed);
        assert!(
            elapsed.as_millis() < 200,
            "Binary search too slow: {:?} (expected <200ms for 10K queries over 100K items)",
            elapsed
        );
    }

    #[test]
    fn test_hot_to_cold_promotion_maintains_sorted_order() {
        let mut mvcc = MVCCManager::new();

        // Fill hot layer to trigger promotion
        // Use timestamps well above GC threshold to avoid premature cleanup
        for i in 0..200_000 {
            mvcc.tombstones.insert(EdgeId(i as u64), 10_000 + (i as u32 % 1000));
        }

        assert!(mvcc.tombstones.len() > HOT_TOMBSTONE_GC_THRESHOLD);

        // Trigger GC which promotes hot to cold
        // Use min_ts that keeps most tombstones (allowing promotion to occur)
        mvcc.gc_tombstones(9_500);

        // Verify cold layer is sorted by EdgeId (required for binary search)
        if mvcc.cold_tombstones.len() > 1 {
            for i in 0..mvcc.cold_tombstones.len() - 1 {
                assert!(
                    mvcc.cold_tombstones[i].0 < mvcc.cold_tombstones[i + 1].0,
                    "Cold layer not sorted at index {}",
                    i
                );
            }
        }

        // Verify we can still use binary search after promotion
        if mvcc.cold_tombstones.len() > 0 {
            let first_edge_id = mvcc.cold_tombstones[0].0;
            assert!(mvcc.is_tombstoned_cold(first_edge_id, u32::MAX));
        }
    }

    #[test]
    fn test_hot_layer_with_cold_layer_integration() {
        let mut mvcc = MVCCManager::new();

        // Add to hot layer
        mvcc.tombstones.insert(EdgeId(1), 100);
        mvcc.tombstones.insert(EdgeId(2), 150);

        // Add to cold layer (pre-sorted)
        mvcc.cold_tombstones = vec![(EdgeId(10), 200), (EdgeId(20), 250)];

        // Test queries across both layers
        assert!(mvcc.is_tombstoned(EdgeId(1), u32::MAX)); // Hot layer
        assert!(mvcc.is_tombstoned(EdgeId(10), u32::MAX)); // Cold layer via binary search
        assert!(!mvcc.is_tombstoned(EdgeId(999), u32::MAX)); // Neither layer

        // Verify GC doesn't break cold layer sort
        mvcc.gc_tombstones(120);
        for i in 0..mvcc.cold_tombstones.len() - 1 {
            assert!(mvcc.cold_tombstones[i].0 < mvcc.cold_tombstones[i + 1].0);
        }
    }
}

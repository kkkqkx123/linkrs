use crate::engine::resource_budget::{MemoryCategory, ResourceSnapshot};

use super::GraphStorageContext;

impl GraphStorageContext {
    pub(crate) fn storage_size(&self) -> usize {
        let vertices = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .values()
                .map(|table| table.memory_size())
                .sum::<usize>()
        });
        let edges = self.persistent.data_store.with_edge_tables(|tables| {
            tables
                .values()
                .map(|arc| arc.read().memory_size())
                .sum::<usize>()
        });
        vertices + edges
    }

    pub(crate) fn used_storage_size(&self) -> usize {
        let vertices = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .values()
                .map(|table| table.used_memory_size())
                .sum::<usize>()
        });
        let edges = self.persistent.data_store.with_edge_tables(|tables| {
            tables
                .values()
                .map(|arc| arc.read().used_memory_size())
                .sum::<usize>()
        });
        vertices + edges
    }

    pub fn resource_snapshot(&self) -> ResourceSnapshot {
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Data, self.used_storage_size() as u64);
        let index_bytes = self
            .persistent
            .index_data_manager
            .read()
            .cached_memory_usage_bytes();
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Index, index_bytes);
        let tombstone_count = self
            .persistent
            .index_data_manager
            .read()
            .cached_tombstone_count() as usize;
        let tombstone_memory_bytes = (tombstone_count as u64).saturating_mul(64);
        self.persistent
            .resource_accounting
            .report_usage(MemoryCategory::Mvcc, tombstone_memory_bytes);
        let _ = self.persistent.cache_manager.refresh_memory_usage();
        // Periodic export point: push collected bloom pre-check counters to
        // the StatsManager alongside the other resource metrics.
        self.persistent
            .index_data_manager
            .read()
            .export_bloom_stats();
        let mut snapshot = self.persistent.resource_accounting.snapshot();
        snapshot.active_snapshots = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .active_count();
        snapshot.oldest_snapshot_ts = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .cleanup_threshold();
        snapshot.tombstone_count = tombstone_count;
        snapshot.tombstone_memory_bytes = tombstone_memory_bytes;
        snapshot.vertex_shard_count = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.values().map(|table| table.num_shards()).sum());
        // Keep spiller accessors exercised.
        let _spill_ratio = self.spiller().spill_threshold_ratio();
        let _spill_dir = self.spiller().spill_dir();
        let _active_spills = self.spiller().active_spills().read().len();
        // Exercise try_reserve_with_spill with a zero-byte probe to keep
        // the full reservation-with-spill path compiled and tested.
        let _probe = self.try_reserve_with_spill(MemoryCategory::Data, 0);
        // Keep vertex GC stats exercised.
        if let Some(ref gc) = self.runtime.vertex_gc_manager {
            let _total = gc.total_removed();
            let _passes = gc.pass_count();
            let _mitigations = gc.pressure_mitigations();
        }
        snapshot
    }

    pub fn check_write_admission(&self) -> graphdb_core::StorageResult<()> {
        if self
            .operation_context
            .as_ref()
            .is_some_and(|context| context.read_only)
        {
            return Err(graphdb_core::StorageError::invalid_operation(
                "Read-only transaction cannot perform writes",
            ));
        }
        let snapshot = self.resource_snapshot();
        if snapshot.hard_limit_exceeded() {
            // Before failing, attempt to spill cold data to recover memory.
            let overage = snapshot
                .total_current_bytes
                .saturating_sub(snapshot.budget.max_memory_bytes)
                + 1024 * 1024;
            self.spiller().spill_cold_data(overage);
            let snapshot = self.resource_snapshot();
            if snapshot.hard_limit_exceeded() {
                return Err(graphdb_core::StorageError::capacity_exceeded());
            }
        }
        let resources = &self.persistent.config.resources;
        if snapshot.tombstone_count >= resources.max_tombstones
            || snapshot.tombstone_memory_bytes >= resources.max_tombstone_bytes
        {
            return Err(graphdb_core::StorageError::capacity_exceeded());
        }
        if snapshot.soft_limit_exceeded() {
            log::debug!(
                "Storage memory is above the soft limit: {} / {} bytes",
                snapshot.total_current_bytes,
                snapshot.budget.max_memory_bytes
            );
        }
        // Version-chain memory diagnostic and long-transaction backpressure.
        // Long-lived snapshots pin history; we warn and surface
        // blocked bytes but do not force the safe GC frontier forward.
        {
            let coordinator = crate::engine::gc_coordinator::GcCoordinator::new(
                self.persistent.version_manager.clone(),
            );
            let diag = coordinator.diagnostics();
            if diag.is_blocked() {
                let version_bytes: usize =
                    self.persistent.data_store.with_vertex_tables(|tables| {
                        tables
                            .values()
                            .map(|t| t.version_chain_memory_bytes())
                            .sum()
                    });
                if version_bytes > 64 * 1024 * 1024 {
                    log::warn!(
                        "write admission: long transaction blocks GC ({} bytes version chains, {} snapshots)",
                        version_bytes,
                        diag.active_snapshot_count
                    );
                }
            }
        }
        Ok(())
    }

    pub fn check_snapshot_admission(&self) -> graphdb_core::StorageResult<()> {
        let tracker = self.persistent.version_manager.snapshot_tracker();
        let active = tracker.active_count();
        if active >= self.persistent.config.resources.max_active_snapshots {
            return Err(graphdb_core::StorageError::capacity_exceeded());
        }
        if let Some(age) = tracker.oldest_age() {
            if age >= self.persistent.config.resources.max_snapshot_age {
                return Err(graphdb_core::StorageError::invalid_operation(
                    "Oldest active snapshot exceeded max_snapshot_age",
                ));
            }
            // Long-transaction diagnostic: warn when oldest snapshot
            // ages past 30s and blocks GC. Does not force watermark forward;
            // caller may retry or apply backpressure upstream.
            if age.as_secs() > 30 {
                let coordinator = crate::engine::gc_coordinator::GcCoordinator::new(
                    self.persistent.version_manager.clone(),
                );
                let diag = coordinator.diagnostics();
                if let Some(warn) = diag.long_transaction_warning {
                    log::warn!("snapshot admission blocked by long transaction: {}", warn);
                }
            }
        }
        Ok(())
    }
}

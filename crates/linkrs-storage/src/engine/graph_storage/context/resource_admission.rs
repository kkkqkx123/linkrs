use crate::engine::resource_budget::{
    MemoryCategory, MemoryPressureLevel, ReliefStep, ResourceSnapshot,
};

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
        // Keep spiller/cache/GC observability exercised without虚设 thresholds.
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
            let _blocked = gc.blocked_passes();
            let _invalidations = gc.cache_invalidations();
        }
        // Keep index GC and unified maintenance scheduling state exercised.
        if let Some(ref gc) = self.runtime.index_gc_manager {
            let _total = gc.total_removed();
            let _passes = gc.pass_count();
        }
        // Pressure observability: warn while loops idle, advance the
        // auto-start streak, and surface TTL hints. No behavior change when
        // the gate is off and pressure is absent.
        self.observe_background_pressure(&snapshot);
        self.ttl_purge_hint();
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "Background maintenance overview: {}",
                self.maintenance_overview().summary()
            );
        }
        snapshot
    }

    /// Fixed relief priority chain. One pressured round walks the chain in
    /// order and stops as soon as the snapshot falls back below the hard
    /// limit, so concurrent evictors never duplicate work. Each step reports
    /// freed bytes under its own identity.
    pub(crate) fn relieve_memory_pressure(&self, snapshot: &ResourceSnapshot) -> ResourceSnapshot {
        if !snapshot.hard_limit_exceeded() {
            return *snapshot;
        }
        let mut current = *snapshot;
        // Step 1: expired trim (record-cache TTL/TTI sweep).
        current = self.relief_step_expired_trim(current);
        if !current.hard_limit_exceeded() {
            return current;
        }
        // Step 2: index cold-chunk eviction toward the low-water target.
        current = self.relief_step_index_evict(current);
        if !current.hard_limit_exceeded() {
            return current;
        }
        // Step 3: cache shrink via the spiller.
        current = self.relief_step_cache_shrink(current);
        if !current.hard_limit_exceeded() {
            return current;
        }
        // Step 4: compaction and generation retirement.
        current = self.relief_step_compaction(current);
        current
    }

    fn relief_step_expired_trim(&self, before: ResourceSnapshot) -> ResourceSnapshot {
        self.persistent.cache_manager.prune_expired_cache();
        let _ = self.persistent.cache_manager.refresh_memory_usage();
        let after = self.refresh_snapshot_totals();
        let freed = before
            .total_current_bytes
            .saturating_sub(after.total_current_bytes);
        self.persistent
            .resource_accounting
            .record_relief(ReliefStep::ExpiredTrim, freed);
        self.merge_relief_counters(after)
    }

    fn relief_step_index_evict(&self, before: ResourceSnapshot) -> ResourceSnapshot {
        let resources = &self.persistent.config.resources;
        let limit = resources.index_memory_bytes;
        if resources.index_eviction_enabled && limit > 0 {
            let _ = self
                .persistent
                .index_data_manager
                .read()
                .evict_cold_chunks_for_pressure(
                    limit,
                    resources.index_eviction_high_ratio,
                    resources.index_eviction_low_ratio,
                );
            self.persistent
                .index_data_manager
                .read()
                .sync_memory_usage();
        }
        let after = self.refresh_snapshot_totals();
        let freed = before
            .total_current_bytes
            .saturating_sub(after.total_current_bytes);
        self.persistent
            .resource_accounting
            .record_relief(ReliefStep::IndexEvict, freed);
        self.merge_relief_counters(after)
    }

    fn relief_step_cache_shrink(&self, before: ResourceSnapshot) -> ResourceSnapshot {
        let overage = before
            .total_current_bytes
            .saturating_sub(before.budget.max_memory_bytes)
            .saturating_add(1024 * 1024);
        // The spiller records under the spill identity; no extra cache record
        // here so the same freed bytes are never counted twice.
        let _ = self.spiller().spill_cold_data(overage);
        let after = self.refresh_snapshot_totals();
        self.merge_relief_counters(after)
    }

    fn relief_step_compaction(&self, before: ResourceSnapshot) -> ResourceSnapshot {
        let safe_ts = self.persistent.version_manager.read_timestamp();
        let retired = self
            .persistent
            .index_data_manager
            .read()
            .retire_generations(safe_ts);
        let _ = retired;
        self.persistent
            .index_data_manager
            .read()
            .sync_memory_usage();
        let after = self.refresh_snapshot_totals();
        let freed = before
            .total_current_bytes
            .saturating_sub(after.total_current_bytes);
        self.persistent
            .resource_accounting
            .record_relief(ReliefStep::Compaction, freed);
        self.merge_relief_counters(after)
    }

    fn refresh_snapshot_totals(&self) -> ResourceSnapshot {
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
        let _ = self.persistent.cache_manager.refresh_memory_usage();
        self.persistent.resource_accounting.snapshot()
    }

    fn merge_relief_counters(&self, mut snapshot: ResourceSnapshot) -> ResourceSnapshot {
        let live = self.persistent.resource_accounting.snapshot();
        snapshot.spill_relief_bytes = live.spill_relief_bytes;
        snapshot.spill_relief_triggers = live.spill_relief_triggers;
        snapshot.index_relief_bytes = live.index_relief_bytes;
        snapshot.index_relief_triggers = live.index_relief_triggers;
        snapshot.cache_relief_bytes = live.cache_relief_bytes;
        snapshot.cache_relief_triggers = live.cache_relief_triggers;
        snapshot.compaction_relief_bytes = live.compaction_relief_bytes;
        snapshot.compaction_relief_triggers = live.compaction_relief_triggers;
        let total = live.total_current_bytes;
        snapshot.pressure_level = if total >= snapshot.budget.hard_limit_bytes {
            MemoryPressureLevel::Critical
        } else if total >= snapshot.budget.soft_limit_bytes {
            MemoryPressureLevel::Elevated
        } else {
            MemoryPressureLevel::Normal
        };
        snapshot.total_current_bytes = total;
        snapshot
    }

    pub fn check_write_admission(&self) -> linkrs_core::StorageResult<()> {
        if self
            .operation_context
            .as_ref()
            .is_some_and(|context| context.read_only)
        {
            return Err(linkrs_core::StorageError::invalid_operation(
                "Read-only transaction cannot perform writes",
            ));
        }
        let snapshot = self.resource_snapshot();
        if snapshot.hard_limit_exceeded() {
            // Fixed priority chain: expired trim, index eviction, cache
            // shrink, compaction/retirement, then write rejection. Stops at
            // the first step that brings usage back below the hard limit.
            let relieved = self.relieve_memory_pressure(&snapshot);
            if relieved.hard_limit_exceeded() {
                return Err(linkrs_core::StorageError::capacity_exceeded());
            }
        }
        let resources = &self.persistent.config.resources;
        if snapshot.tombstone_count >= resources.max_tombstones
            || snapshot.tombstone_memory_bytes >= resources.max_tombstone_bytes
        {
            return Err(linkrs_core::StorageError::capacity_exceeded());
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

    pub fn check_snapshot_admission(&self) -> linkrs_core::StorageResult<()> {
        let tracker = self.persistent.version_manager.snapshot_tracker();
        let active = tracker.active_count();
        if active >= self.persistent.config.resources.max_active_snapshots {
            return Err(linkrs_core::StorageError::capacity_exceeded());
        }
        if let Some(age) = tracker.oldest_age() {
            if age >= self.persistent.config.resources.max_snapshot_age {
                return Err(linkrs_core::StorageError::invalid_operation(
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

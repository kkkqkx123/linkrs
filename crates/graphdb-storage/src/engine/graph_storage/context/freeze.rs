use crate::edge::edge_table::segment_eviction::SegmentEvictionEngine;
use crate::engine::background_freeze::{FreezeGuard, FreezeStats};
use graphdb_core::types::{AutoCompactConfig, CompactConfig, Timestamp};
use graphdb_core::StorageResult;
use parking_lot::Mutex;
use std::sync::Arc;

use super::GraphStorageContext;

impl GraphStorageContext {
    /// Pure decision predicate for automatic vertex compaction: enabled,
    /// absolute hole count above `min_holes`, and hole ratio at or above
    /// `min_hole_ratio` (holes / allocated IDs).
    pub(crate) fn should_auto_compact(
        live: usize,
        allocated: usize,
        cfg: &AutoCompactConfig,
    ) -> bool {
        if !cfg.enable_vertex_compaction {
            return false;
        }
        let holes = allocated.saturating_sub(live);
        if holes < cfg.min_holes as usize {
            return false;
        }
        if allocated > 0 && (holes as f32) / (allocated as f32) < cfg.min_hole_ratio {
            return false;
        }
        true
    }

    /// Schedule the background maintenance task on the shared thread pool:
    /// automatic vertex compaction (if ID holes exceed thresholds) followed
    /// by delta freeze. No-op while a previous maintenance run is in flight.
    pub(crate) fn schedule_background_maintenance(&self) {
        if self
            .runtime
            .background_freeze_running
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }

        let context = self.clone();
        let running = self.runtime.background_freeze_running.clone();
        let timeout = self.persistent.config.resources.operation_timeout;
        self.runtime.thread_pool.spawn(move || {
            let started = std::time::Instant::now();
            if let Err(error) = context.trigger_background_maintenance() {
                log::warn!("Background maintenance failed: {}", error);
            }
            if started.elapsed() > timeout {
                log::warn!(
                    "Background maintenance exceeded operation timeout: {:?} > {:?}",
                    started.elapsed(),
                    timeout
                );
            }
            running.store(false, std::sync::atomic::Ordering::Release);
        });
    }

    /// Run background maintenance synchronously: automatic vertex compaction
    /// followed by the existing delta freeze pass, then per-table automatic
    /// maintenance (tombstone GC, property compaction, delta freeze).
    /// Captures watermarks once and shares across all sub-passes.
    pub(crate) fn trigger_background_maintenance(&self) -> StorageResult<()> {
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        let wm = gc.capture_watermarks();
        if let Err(e) = self.maybe_auto_compact_vertices_with_watermarks(&wm) {
            log::warn!("Automatic vertex compaction failed: {}", e);
        }
        self.trigger_background_freeze_with_watermarks(&wm)?;
        self.trigger_auto_edge_maintenance_with_watermarks(&wm)
    }

    /// Run the storage-level automatic maintenance pass on every edge
    /// partition: tombstone GC, property compaction, and delta freeze
    /// based on each table's configured thresholds.
    /// Uses the unified `GcCoordinator` watermark so the cutoff is shared
    /// with vertex and index GC in the same epoch.
    fn trigger_auto_edge_maintenance(&self) -> StorageResult<()> {
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        let wm = gc.capture_watermarks();
        self.trigger_auto_edge_maintenance_with_watermarks(&wm)
    }

    fn trigger_auto_edge_maintenance_with_watermarks(
        &self,
        wm: &graphdb_transaction::MvccWatermarks,
    ) -> StorageResult<()> {
        let margin = self.persistent.config.gc_safety_margin;
        self.persistent
            .data_store
            .for_all_edge_partitions_mut(|_key, table| {
                let ran = table.maybe_run_auto_maintenance_with_watermarks(wm, margin);
                if ran > 0 && log::log_enabled!(log::Level::Debug) {
                    let stats = table.tombstone_stats();
                    log::debug!(
                        "Auto edge maintenance (watermark) ran {} passes on {} (tombstones={})",
                        ran,
                        table.label(),
                        stats.count
                    );
                }
                Ok(())
            })?;
        Ok(())
    }

    /// Set the operator retention floor on every edge partition.
    ///
    /// This is the reclamation exit for the no-snapshot steady state: with a
    /// floor at `ts`, deletions at or before `ts` become reclaimable by the
    /// regular maintenance pipeline exactly as if a snapshot existed at that
    /// point. Registered snapshots always win — the floor only applies while
    /// no snapshot pins history. A floor of `0` disables it again.
    pub fn set_edge_retention_floor(&self, ts: Timestamp) -> StorageResult<()> {
        self.persistent
            .data_store
            .for_all_edge_partitions_mut(|_key, table| {
                table.set_retention_floor(ts);
                Ok(())
            })?;
        log::info!("Edge retention floor set to {} on all partitions", ts);
        Ok(())
    }

    /// Compact vertex tables whose deleted-vertex ID holes exceed the
    /// configured thresholds (absolute count and ratio), with a cooldown
    /// between runs. Deletions at or before the snapshot cleanup threshold
    /// are reclaimed so active snapshot time-travel stays intact.
    fn maybe_auto_compact_vertices(&self) -> StorageResult<()> {
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        let wm = gc.capture_watermarks();
        self.maybe_auto_compact_vertices_with_watermarks(&wm)
    }

    fn maybe_auto_compact_vertices_with_watermarks(
        &self,
        wm: &graphdb_transaction::MvccWatermarks,
    ) -> StorageResult<()> {
        let cfg = &self.persistent.config.auto_compact;
        if !cfg.enable_vertex_compaction {
            return Ok(());
        }

        let now = std::time::Instant::now();
        {
            let last = self.runtime.last_auto_compact.lock();
            if let Some(prev) = *last {
                if now.duration_since(prev).as_secs() < cfg.min_interval_secs {
                    return Ok(());
                }
            }
        }

        let (live, allocated) = {
            let safe_ts = wm.safe_gc_timestamp();
            self.persistent.data_store.with_vertex_tables(|tables| {
                let mut live = 0;
                let mut allocated = 0;
                for table in tables.values() {
                    let (l, a) = table.id_hole_stats(safe_ts);
                    live += l;
                    allocated += a;
                }
                (live, allocated)
            })
        };

        if !Self::should_auto_compact(live, allocated, cfg) {
            return Ok(());
        }

        let cleanup_ts = wm.safe_gc_timestamp();
        let removed = self.compact_vertex_remap(cleanup_ts)?;
        *self.runtime.last_auto_compact.lock() = Some(std::time::Instant::now());
        log::info!(
            "Automatic vertex compaction removed {} vertices (holes={}, live={})",
            removed,
            allocated.saturating_sub(live),
            live
        );
        Ok(())
    }

    pub(crate) fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.runtime
            .background_freeze_manager
            .as_ref()
            .map(|m| m.get_stats())
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        let gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        let wm = gc.capture_watermarks();
        self.trigger_background_freeze_with_watermarks(&wm)
    }

    fn trigger_background_freeze_with_watermarks(
        &self,
        wm: &graphdb_transaction::MvccWatermarks,
    ) -> StorageResult<()> {
        // Reserve ratio 0.5 doubles the compacted capacity (matches the
        // original 2.0 growth intent; 2.0 clamps to 1.0 inside
        // `with_fixed_ratio` and would divide by zero in the CSR rebuild).
        let config = CompactConfig::with_fixed_ratio(true, 0.5).enable_segment_merge(1000);
        // Freeze incrementally up to the unified GC watermark so all table
        // types in the same pass share the same cutoff and no prefix reclaim
        // can change the cutoff for a later type in the same pass.
        let ts = wm.safe_gc_timestamp();

        // Use FreezeGuard to manage freeze statistics
        let mut freeze_guard = self
            .runtime
            .background_freeze_manager
            .as_ref()
            .map(|m| FreezeGuard::new(m.clone()));

        let totals = Arc::new(Mutex::new((0u64, false, std::collections::HashSet::new())));

        self.persistent
            .data_store
            .for_all_edge_partitions_mut(|_key, table| {
                let delta_edges = table.delta_edge_count();
                let delta_memory = table.used_memory_size() as u64;
                let mut frozen_here = 0u64;
                let mut any_here = false;

                if let Some(ref manager) = self.runtime.background_freeze_manager {
                    manager.record_delta_size(delta_edges);

                    let input = crate::engine::config::FreezeDecisionInput {
                        delta_edge_count: delta_edges,
                        delta_memory_bytes: delta_memory,
                        segment_count: 0,
                        oldest_segment_age: 0,
                        deletion_ratio: 0.0,
                    };

                    if manager.should_freeze_with_stats(&input) {
                        let decision = manager.get_freeze_decision_with_stats(&input);
                        let mut t = totals.lock();
                        t.2.insert(decision.freeze_reason);
                        log::debug!(
                            "Freeze triggered ({} strategy): {}",
                            manager.strategy_name(),
                            decision.summary()
                        );

                        frozen_here = table.compact_and_freeze(ts, &config) as u64;
                        any_here = true;
                    } else if log::log_enabled!(log::Level::Debug) {
                        log::debug!(
                            "Skip freeze ({} strategy): {}",
                            manager.strategy_name(),
                            manager.get_reason(&input)
                        );
                    }
                } else {
                    if delta_edges >= self.persistent.config.freeze.delta_edge_threshold {
                        frozen_here = table.compact_and_freeze(ts, &config) as u64;
                        any_here = true;
                    }
                }
                let mut t = totals.lock();
                t.0 += frozen_here;
                t.1 |= any_here;
                Ok(())
            })?;

        let (total_frozen, any_frozen, freeze_reasons) = {
            let t = totals.lock();
            (t.0, t.1, t.2.clone())
        };

        if any_frozen {
            // Record freeze via guard (automatically logged on drop)
            if let Some(ref mut guard) = freeze_guard {
                guard.record_edges(total_frozen);
            } else {
                // Fallback manual recording if no manager
                if let Some(ref manager) = self.runtime.background_freeze_manager {
                    let duration_ms = 0;
                    manager.record_freeze(total_frozen, duration_ms);
                }
            }

            if let Some(ref manager) = self.runtime.background_freeze_manager {
                let reason_str = if freeze_reasons.is_empty() {
                    "none".to_string()
                } else {
                    freeze_reasons
                        .iter()
                        .map(|r| match r {
                            crate::engine::background_freeze::FreezeReason::EdgeCountExceeded => {
                                "edges"
                            }
                            crate::engine::background_freeze::FreezeReason::MemoryExceeded => {
                                "memory"
                            }
                            crate::engine::background_freeze::FreezeReason::Both => "edges+memory",
                            crate::engine::background_freeze::FreezeReason::None => "none",
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };

                log::info!(
                    "Background freeze ({} strategy): {} edges frozen (reason: {})",
                    manager.strategy_name(),
                    total_frozen,
                    reason_str
                );
            }
        }

        // Automatic cold-hot tiering after the delta-freeze pass so both
        // freeze operations run serially under each table's write lock.
        if let Err(err) = self.maybe_freeze_cold_tier() {
            log::warn!("Cold-tier freeze evaluation failed: {}", err);
        }

        Ok(())
    }

    /// Check if memory pressure exceeds the soft limit and evict cold segments if needed.
    pub fn trigger_segment_eviction(&self) -> StorageResult<u64> {
        let accounting = &self.persistent.resource_accounting;
        let snapshot = accounting.snapshot();

        if !snapshot.soft_limit_exceeded() {
            return Ok(0);
        }

        let excess = snapshot
            .total_current_bytes
            .saturating_sub(snapshot.budget.soft_limit_bytes);
        if excess == 0 {
            return Ok(0);
        }

        let target_bytes = excess as usize;
        let mut total_freed: u64 = 0;

        let spill_dir = self.persistent.layout.spill_dir();
        std::fs::create_dir_all(&spill_dir)?;

        let engine = SegmentEvictionEngine::new(spill_dir);

        self.persistent.data_store.with_edge_tables(|edge_tables| {
            for arc in edge_tables.values() {
                if total_freed >= excess {
                    break;
                }
                let remaining = excess - total_freed;
                let table = arc.read();
                match engine.evict_cold_segments(&table, remaining as usize) {
                    Ok(freed) => total_freed += freed as u64,
                    Err(e) => {
                        log::warn!("Segment eviction failed for table: {}", e);
                    }
                }
            }
        });

        if total_freed > 0 {
            accounting.release(
                crate::engine::resource_budget::MemoryCategory::Data,
                total_freed,
            );
            log::info!(
                "Segment eviction freed {} bytes (target: {} bytes)",
                total_freed,
                target_bytes
            );
            // Evicting cold segments changes the physical edge layout.
            self.bump_layout_version();
        }

        Ok(total_freed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AutoCompactConfig {
        AutoCompactConfig {
            enable_vertex_compaction: true,
            min_holes: 100,
            min_hole_ratio: 0.25,
            min_interval_secs: 3600,
        }
    }

    #[test]
    fn test_should_auto_compact_below_absolute_threshold() {
        assert!(!GraphStorageContext::should_auto_compact(
            1000,
            1050,
            &cfg()
        ));
    }

    #[test]
    fn test_should_auto_compact_below_ratio_threshold() {
        assert!(!GraphStorageContext::should_auto_compact(
            9000,
            10_000,
            &cfg()
        ));
    }

    #[test]
    fn test_should_auto_compact_above_both_thresholds() {
        assert!(GraphStorageContext::should_auto_compact(
            7000,
            10_000,
            &cfg()
        ));
    }

    #[test]
    fn test_should_auto_compact_disabled() {
        let mut c = cfg();
        c.enable_vertex_compaction = false;
        assert!(!GraphStorageContext::should_auto_compact(0, 10_000, &c));
    }

    #[test]
    fn test_should_auto_compact_empty_table() {
        assert!(!GraphStorageContext::should_auto_compact(0, 0, &cfg()));
    }
}

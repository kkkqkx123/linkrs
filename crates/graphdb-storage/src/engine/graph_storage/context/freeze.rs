use crate::engine::background_freeze::{FreezeGuard, FreezeStats};
use graphdb_core::types::{AutoCompactConfig, CompactConfig};
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
    /// followed by single-segment edge compaction, then per-table automatic
    /// maintenance (tombstone GC, property compaction).
    /// Captures watermarks once and shares across all sub-passes.
    pub(crate) fn trigger_background_maintenance(&self) -> StorageResult<()> {
        let gc = self.gc_coordinator();
        let wm = gc.capture_watermarks();
        if let Err(e) = self.maybe_auto_compact_vertices_with_watermarks(&wm) {
            log::warn!("Automatic vertex compaction failed: {}", e);
        }
        self.trigger_background_freeze_with_watermarks(&wm)?;
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
                    let stats = table.mvcc.tombstone_stats();
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
        let gc = self.gc_coordinator();
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
        let config = CompactConfig::with_fixed_ratio(true, 0.5);
        // One watermark capture shared by every table in this pass, margin
        // applied to absorb the capture-execute race.
        let margin = self.persistent.config.gc_safety_margin;

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

                    let deletion_ratio = table.deletion_stats().deletion_ratio();
                    // Skip healthy tables: a full CSR rebuild frees nothing
                    // when there are no tombstones and no group is fragmented.
                    // The check is per-group so a fragmented group cannot drag
                    // clean groups into the rebuild set; the whole-table ratio
                    // stays an observation metric (0.5 wasted share is the
                    // documented rebuild-worthy level, applied per group).
                    let needs_reclaim = table.deletion_stats().total_deleted_edges > 0
                        || table.has_fragmented_group(0.5);
                    if !needs_reclaim {
                        return Ok(());
                    }

                    let input = crate::engine::config::FreezeDecisionInput {
                        delta_edge_count: delta_edges,
                        delta_memory_bytes: delta_memory,
                        deletion_ratio,
                    };

                    if manager.should_freeze_with_stats(&input) {
                        let decision = manager.get_freeze_decision_with_stats(&input);
                        let mut t = totals.lock();
                        t.2.insert(decision.freeze_reason);
                        log::debug!("Freeze triggered: {}", decision.summary());

                        let reserve_ratio =
                            config.compute_reserve_ratio(table.edge_count() as usize, 0);
                        frozen_here =
                            table.compact_csr_only_with_watermarks(wm, margin, reserve_ratio)
                                as u64;
                        table.compact_properties_with_watermarks(wm, margin);
                        any_here = true;
                    } else if log::log_enabled!(log::Level::Debug) {
                        log::debug!("Skip freeze: {}", manager.get_reason(&input));
                    }
                } else {
                    if delta_edges >= self.persistent.config.freeze.delta_edge_threshold {
                        let reserve_ratio =
                            config.compute_reserve_ratio(table.edge_count() as usize, 0);
                        frozen_here =
                            table.compact_csr_only_with_watermarks(wm, margin, reserve_ratio)
                                as u64;
                        table.compact_properties_with_watermarks(wm, margin);
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

            if self.runtime.background_freeze_manager.is_some() {
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
                            crate::engine::background_freeze::FreezeReason::DeletionExceeded => {
                                "deletions"
                            }
                            crate::engine::background_freeze::FreezeReason::Both => "edges+memory",
                            crate::engine::background_freeze::FreezeReason::None => "none",
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };

                log::info!(
                    "Background freeze: {} edges frozen (reason: {})",
                    total_frozen,
                    reason_str
                );
            }
        }

        Ok(())
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

    #[test]
    fn test_background_freeze_manager_basics() {
        use crate::engine::background_freeze::BackgroundFreezeManager;
        use crate::engine::config::{FreezeConfig, FreezeDecisionInput};

        let config = FreezeConfig {
            delta_edge_threshold: 1000,
            delta_memory_threshold_bytes: 256 * 1024 * 1024,
            deletion_threshold: 0.5,
        };
        let manager = BackgroundFreezeManager::from_config(config);

        // Test should_freeze decision (only edge count threshold)
        let input1 = FreezeDecisionInput {
            delta_edge_count: 500,
            delta_memory_bytes: 100 * 1024 * 1024,
            deletion_ratio: 0.1,
        };
        assert!(!manager.should_freeze_with_stats(&input1));

        let input2 = FreezeDecisionInput {
            delta_edge_count: 1000,
            ..input1
        };
        assert!(manager.should_freeze_with_stats(&input2));

        let input3 = FreezeDecisionInput {
            delta_edge_count: 1500,
            ..input1
        };
        assert!(manager.should_freeze_with_stats(&input3));

        // Test should_freeze with memory threshold exceeded
        let input4 = FreezeDecisionInput {
            delta_edge_count: 500,
            delta_memory_bytes: 300 * 1024 * 1024,
            deletion_ratio: 0.1,
        };
        assert!(manager.should_freeze_with_stats(&input4));

        // Test should_freeze with deletion threshold exceeded
        let input5 = FreezeDecisionInput {
            deletion_ratio: 0.6,
            ..input1
        };
        assert!(manager.should_freeze_with_stats(&input5));

        // Test record_freeze
        manager.record_freeze(100, 50);
        let stats = manager.get_stats();
        assert_eq!(stats.freeze_count, 1);
        assert_eq!(stats.total_frozen_edges, 100);
        assert_eq!(stats.last_freeze_duration_ms, 50);

        // Test record_delta_size
        manager.record_delta_size(750);
        let stats = manager.get_stats();
        assert_eq!(stats.current_delta_edges, 750);
    }
}

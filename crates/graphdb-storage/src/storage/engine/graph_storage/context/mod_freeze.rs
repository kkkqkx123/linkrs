use crate::core::types::CompactConfig;
use crate::core::StorageResult;
use crate::storage::edge::edge_table::segment_eviction::SegmentEvictionEngine;
use crate::storage::engine::background_freeze::{FreezeGuard, FreezeStats};

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.runtime
            .background_freeze_manager
            .as_ref()
            .map(|m| m.get_stats())
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        let config = CompactConfig::with_fixed_ratio(true, 2.0).enable_segment_merge(1000);
        let ts = u32::MAX;
        let mut total_frozen = 0u64;
        let mut any_frozen = false;
        let mut freeze_reasons = std::collections::HashSet::new();

        // Use FreezeGuard to manage freeze statistics
        let mut freeze_guard = self
            .runtime
            .background_freeze_manager
            .as_ref()
            .map(|m| FreezeGuard::new(m.clone()));

        self.persistent
            .data_store
            .with_edge_tables_mut(|edge_tables| {
                for table in edge_tables.values_mut() {
                    let delta_edges = table.delta_edge_count();
                    let delta_memory = table.used_memory_size() as u64;

                    if let Some(ref manager) = self.runtime.background_freeze_manager {
                        manager.record_delta_size(delta_edges);

                        let input = crate::storage::engine::config::FreezeDecisionInput {
                            delta_edge_count: delta_edges,
                            delta_memory_bytes: delta_memory,
                            segment_count: 0,
                            oldest_segment_age: 0,
                            deletion_ratio: 0.0,
                        };

                        if manager.should_freeze_with_stats(&input) {
                            let decision = manager.get_freeze_decision_with_stats(&input);
                            freeze_reasons.insert(decision.freeze_reason);
                            log::debug!(
                                "Freeze triggered ({} strategy): {}",
                                manager.strategy_name(),
                                decision.summary()
                            );

                            let frozen = table.compact_and_freeze(
                                ts,
                                &config,
                                crate::storage::edge::edge_table::CompactionMode::Standard,
                            );
                            total_frozen += frozen as u64;
                            any_frozen = true;
                        } else if log::log_enabled!(log::Level::Debug) {
                            log::debug!(
                                "Skip freeze ({} strategy): {}",
                                manager.strategy_name(),
                                manager.get_reason(&input)
                            );
                        }
                    } else {
                        if delta_edges >= self.persistent.config.freeze.delta_edge_threshold {
                            let frozen = table.compact_and_freeze(
                                ts,
                                &config,
                                crate::storage::edge::edge_table::CompactionMode::Standard,
                            );
                            total_frozen += frozen as u64;
                            any_frozen = true;
                        }
                    }
                }
                Ok(())
            })?;

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
                            crate::storage::engine::background_freeze::FreezeReason::EdgeCountExceeded => "edges",
                            crate::storage::engine::background_freeze::FreezeReason::MemoryExceeded => "memory",
                            crate::storage::engine::background_freeze::FreezeReason::Both => "edges+memory",
                            crate::storage::engine::background_freeze::FreezeReason::None => "none",
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
            for table in edge_tables.values() {
                if total_freed >= excess {
                    break;
                }
                let remaining = excess - total_freed;
                match engine.evict_cold_segments(table, remaining as usize) {
                    Ok(freed) => total_freed += freed as u64,
                    Err(e) => {
                        log::warn!("Segment eviction failed for table: {}", e);
                    }
                }
            }
        });

        if total_freed > 0 {
            accounting.release(
                crate::storage::engine::resource_budget::MemoryCategory::Data,
                total_freed,
            );
            log::info!(
                "Segment eviction freed {} bytes (target: {} bytes)",
                total_freed,
                target_bytes
            );
        }

        Ok(total_freed)
    }
}

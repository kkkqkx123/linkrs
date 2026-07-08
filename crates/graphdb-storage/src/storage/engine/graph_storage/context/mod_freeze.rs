use crate::core::types::CompactConfig;
use crate::core::StorageResult;
use crate::storage::engine::background_freeze::{FreezeStats, FreezeGuard};

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn get_freeze_stats(&self) -> Option<FreezeStats> {
        self.runtime
            .background_freeze_manager
            .as_ref()
            .map(|m| m.get_stats())
    }

    pub fn trigger_background_freeze(&self) -> StorageResult<()> {
        let config = CompactConfig::with_fixed_ratio(true, 2.0)
            .enable_segment_merge(1000);
        let ts = u32::MAX;
        let mut total_frozen = 0u64;
        let mut any_frozen = false;
        let mut freeze_reasons = std::collections::HashSet::new();

        // Use FreezeGuard to manage freeze statistics
        let mut freeze_guard = self.runtime.background_freeze_manager
            .as_ref()
            .map(|m| FreezeGuard::new(m.clone()));

        {
            let mut edge_tables = self.persistent.data_store.edge_tables().write();
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

                        let frozen = table.compact_and_freeze(ts, &config, crate::storage::edge::edge_table::CompactionMode::Standard);
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
                        let frozen = table.compact_and_freeze(ts, &config, crate::storage::edge::edge_table::CompactionMode::Standard);
                        total_frozen += frozen as u64;
                        any_frozen = true;
                    }
                }
            }
        }

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
}

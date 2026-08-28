use crate::engine::graph_storage::GraphStorageContext;
use graphdb_core::types::{CompactConfig, CompactResult, CompactStats, CompactTarget};
use graphdb_core::types::{CompactError, Timestamp};

impl CompactTarget for GraphStorageContext {
    fn compact(&self, config: &CompactConfig, ts: Timestamp) -> CompactResult<()> {
        let strategy_desc = match &config.strategy {
            graphdb_core::types::CompactionStrategy::Fixed(ratio) => {
                format!("fixed(ratio={})", ratio)
            }
            graphdb_core::types::CompactionStrategy::Adaptive(cfg) => {
                format!(
                    "adaptive(base={}, reduced={}, threshold={})",
                    cfg.base_ratio, cfg.reduced_ratio, cfg.size_threshold
                )
            }
        };
        log::info!(
            "Starting compaction: enable_structure_compaction={}, strategy={}, ts={}",
            config.enable_structure_compaction,
            strategy_desc,
            ts
        );

        self.compact_maintenance(config, ts)
            .map_err(|err| CompactError::StorageError(err.to_string()))
    }

    fn get_compact_stats(&self) -> CompactStats {
        let total = self.storage_size();
        let used = self.used_storage_size();
        CompactStats::new(total, used)
    }
}

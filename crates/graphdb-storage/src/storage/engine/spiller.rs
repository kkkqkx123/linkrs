//! Disk spiller for query scratch space.
//!
//! When memory pressure exceeds the hard limit during query execution, the spiller
//! evicts cold segments and/or cached data to temporary files, freeing physical
//! memory and enabling the allocation to succeed on retry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::StorageResult;
use crate::storage::edge::edge_table::segment_eviction::SegmentEvictionEngine;
use crate::storage::engine::cache_manager::CacheManager;
use crate::storage::engine::data_store::GraphDataStore;
use crate::storage::engine::resource_budget::{
    MemoryAccounting, MemoryCategory, MemoryReservation,
};

/// Metadata for an active spill file.
#[derive(Debug)]
pub struct SpillFile {
    pub path: PathBuf,
    pub category: MemoryCategory,
    pub spilled_bytes: u64,
}

/// Spill manager for query scratch data.
///
/// When memory pressure exceeds the hard limit during `try_reserve`, the spiller
/// evicts cold segments to temporary files and force-evicts cached data if needed,
/// enabling graceful degradation instead of failing with `CapacityExceeded`.
pub struct Spiller {
    spill_dir: PathBuf,
    accounting: Arc<MemoryAccounting>,
    data_store: Arc<GraphDataStore>,
    cache_manager: Arc<CacheManager>,
    active_spills: RwLock<Vec<SpillFile>>,
    /// Ratio of hard_limit at which proactive spill triggers.
    spill_threshold_ratio: f64,
}

impl Spiller {
    pub fn new(
        spill_dir: PathBuf,
        accounting: Arc<MemoryAccounting>,
        data_store: Arc<GraphDataStore>,
        cache_manager: Arc<CacheManager>,
        spill_threshold_ratio: f64,
    ) -> Self {
        Self {
            spill_dir,
            accounting,
            data_store,
            cache_manager,
            active_spills: RwLock::new(Vec::new()),
            spill_threshold_ratio,
        }
    }

    /// Attempt a memory reservation, spilling cold data on hard-limit failure.
    pub fn try_reserve_with_spill(
        &self,
        category: MemoryCategory,
        bytes: u64,
    ) -> StorageResult<MemoryReservation> {
        let spiller = self.clone();
        self.accounting
            .try_reserve_with_spill(category, bytes, move || spiller.spill_cold_data(bytes))
    }

    /// Spill cold data to free memory.
    ///
    /// Evicts cold segments first, then force-evicts from cache if still under pressure.
    /// Returns the number of bytes freed (0 if nothing could be spilled).
    pub fn spill_cold_data(&self, requested_bytes: u64) -> Option<u64> {
        if requested_bytes == 0 {
            return None;
        }

        if let Err(e) = std::fs::create_dir_all(&self.spill_dir) {
            log::warn!(
                "Failed to create spill directory {}: {}",
                self.spill_dir.display(),
                e
            );
            return None;
        }

        let engine = SegmentEvictionEngine::new(self.spill_dir.clone());
        let mut total_freed: u64 = 0;

        self.data_store.with_edge_tables(|edge_tables| {
            for arc in edge_tables.values() {
                if total_freed >= requested_bytes {
                    break;
                }
                let remaining = (requested_bytes - total_freed) as usize;
                let table = arc.read();
                match engine.evict_cold_segments(&table, remaining) {
                    Ok(freed) => total_freed += freed as u64,
                    Err(e) => {
                        log::warn!("Segment eviction failed during spill: {}", e);
                    }
                }
            }
        });

        if total_freed >= requested_bytes {
            self.active_spills.write().push(SpillFile {
                path: self.spill_dir.join("segment_eviction.spill"),
                category: MemoryCategory::Data,
                spilled_bytes: total_freed,
            });
            self.accounting.release(MemoryCategory::Data, total_freed);
            return Some(total_freed);
        }

        let snapshot = self.accounting.snapshot();
        let cache_bytes = snapshot.categories[MemoryCategory::Cache as usize].current_bytes;
        if cache_bytes > 0 {
            // Shrink cache capacity to evict cold entries; BufferPool eviction
            // reports freed bytes back to the accounting, so re-report the
            // remaining usage for an accurate freed delta.
            self.cache_manager.shrink_cache();
            self.cache_manager.refresh_memory_usage();
            let after = self.accounting.snapshot();
            let freed = cache_bytes.saturating_sub(
                after.categories[MemoryCategory::Cache as usize].current_bytes,
            );
            if freed > 0 {
                self.active_spills.write().push(SpillFile {
                    path: self.spill_dir.join("cache_eviction.spill"),
                    category: MemoryCategory::Cache,
                    spilled_bytes: freed,
                });
                total_freed += freed;
            }
        }

        if total_freed > 0 {
            self.accounting.release(MemoryCategory::Data, total_freed);
            Some(total_freed)
        } else {
            None
        }
    }

    /// Clean up stale spill files from a previous run.
    pub fn cleanup_stale_files(&self) -> StorageResult<()> {
        if !self.spill_dir.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&self.spill_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("spill") {
                std::fs::remove_file(&path)?;
                log::debug!("Removed stale spill file: {}", path.display());
            }
        }
        Ok(())
    }

    pub fn spill_dir(&self) -> &Path {
        &self.spill_dir
    }

    pub fn active_spills(&self) -> &RwLock<Vec<SpillFile>> {
        &self.active_spills
    }

    pub fn spill_threshold_ratio(&self) -> f64 {
        self.spill_threshold_ratio
    }
}

impl Clone for Spiller {
    fn clone(&self) -> Self {
        Self {
            spill_dir: self.spill_dir.clone(),
            accounting: Arc::clone(&self.accounting),
            data_store: Arc::clone(&self.data_store),
            cache_manager: Arc::clone(&self.cache_manager),
            active_spills: RwLock::new(Vec::new()),
            spill_threshold_ratio: self.spill_threshold_ratio,
        }
    }
}

impl Drop for Spiller {
    fn drop(&mut self) {
        let spills = self.active_spills.read();
        for spill in spills.iter() {
            let _cat = spill.category;
            let _bytes = spill.spilled_bytes;
            log::debug!(
                "Cleaning up spill {} ({:?}, {} bytes)",
                spill.path.display(),
                _cat,
                _bytes,
            );
            if let Err(e) = std::fs::remove_file(&spill.path) {
                log::warn!(
                    "Failed to remove spill file {}: {}",
                    spill.path.display(),
                    e
                );
            }
        }
    }
}

impl std::fmt::Debug for Spiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spiller")
            .field("spill_dir", &self.spill_dir)
            .field("spill_threshold_ratio", &self.spill_threshold_ratio)
            .field("active_spills", &self.active_spills.read().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spiller_debug_info() {
        let accounting = Arc::new(MemoryAccounting::new(
            crate::storage::engine::resource_budget::MemoryBudget::from_validated(
                1024, 256, 0.8, 0.95,
            ),
        ));
        let data_store = Arc::new(GraphDataStore::new());
        let cache_manager = Arc::new(CacheManager::new(
            false,
            0,
            &crate::storage::engine::config::ResourceConfig::default(),
            Arc::clone(&accounting),
        ));
        let spiller = Spiller::new(
            PathBuf::from("/tmp/linkrs_test_spill"),
            accounting,
            data_store,
            cache_manager,
            0.90,
        );
        let debug = format!("{:?}", spiller);
        assert!(debug.contains("Spiller"));
        assert!(debug.contains("0.9"));
    }
}

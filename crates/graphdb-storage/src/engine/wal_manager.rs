//! WAL Manager
//!
//! Unified WAL (Write-Ahead Log) manager that properly integrates with LocalWalWriter.
//! This module provides a single source of truth for LSN management and WAL operations.

use crate::core::types::{CommitLsn, Timestamp, TransactionId};
use crate::core::wal::types::WalOpType;
use crate::core::wal::OutboxIntent;
use crate::core::{StorageError, StorageResult};
use crate::index::shard_runtime::IndexBarrierRegistry;
use crate::transaction::wal::writer::WalWriter;
use crate::transaction::wal::TransactionWalEntry;
use crate::transaction::wal::{LocalWalWriter, Lsn, WalConfig};
use parking_lot::Mutex;
use postcard::to_allocvec;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Unified WAL manager that wraps LocalWalWriter
///
/// This manager ensures LSN consistency by delegating all LSN operations
/// to the underlying LocalWalWriter, avoiding the dual LSN tracking issue.
pub struct WalManager {
    local_writer: Option<Arc<Mutex<LocalWalWriter>>>,
    barrier_registry: Option<IndexBarrierRegistry>,
    config: WalConfig,
    sync_count: AtomicU64,
    sync_failures: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalMetrics {
    pub accepted_lsn: Lsn,
    pub durable_lsn: Lsn,
    pub sync_count: u64,
    pub sync_failures: u64,
}

impl WalManager {
    pub fn new() -> Self {
        Self {
            local_writer: None,
            barrier_registry: None,
            config: WalConfig::default(),
            sync_count: AtomicU64::new(0),
            sync_failures: AtomicU64::new(0),
        }
    }

    pub fn with_config(config: WalConfig) -> Self {
        Self {
            local_writer: None,
            barrier_registry: None,
            config,
            sync_count: AtomicU64::new(0),
            sync_failures: AtomicU64::new(0),
        }
    }

    pub fn open(&mut self, wal_dir: &Path, thread_id: u32) -> StorageResult<()> {
        let wal_uri = wal_dir.to_string_lossy().to_string();
        let mut writer = LocalWalWriter::with_config(&wal_uri, thread_id, self.config.clone());
        writer
            .open()
            .map_err(|e| StorageError::wal_error(format!("Failed to open WAL: {:?}", e)))?;
        if self.config.group_commit_enabled {
            writer.enable_group_commit().map_err(|e| {
                StorageError::wal_error(format!("Failed to enable group commit: {:?}", e))
            })?;
        }
        self.local_writer = Some(Arc::new(Mutex::new(writer)));
        Ok(())
    }

    pub(crate) fn set_index_barrier_registry(&mut self, registry: IndexBarrierRegistry) {
        self.barrier_registry = Some(registry);
    }

    fn truncation_barrier_lsn(&self) -> Option<Lsn> {
        self.barrier_registry.as_ref().and_then(|registry| {
            registry
                .read()
                .values()
                .map(|barrier| Lsn::new(barrier.get()))
                .min()
        })
    }

    pub fn current_lsn(&self) -> Lsn {
        if let Some(ref writer) = self.local_writer {
            writer.lock().current_lsn()
        } else {
            Lsn::ZERO
        }
    }

    /// Return the latest WAL position confirmed durable by the writer.
    pub fn durable_lsn(&self) -> Lsn {
        if let Some(ref writer) = self.local_writer {
            writer.lock().durable_lsn()
        } else {
            Lsn::ZERO
        }
    }

    pub fn metrics(&self) -> WalMetrics {
        WalMetrics {
            accepted_lsn: self.current_lsn(),
            durable_lsn: self.durable_lsn(),
            sync_count: self.sync_count.load(Ordering::Relaxed),
            sync_failures: self.sync_failures.load(Ordering::Relaxed),
        }
    }

    pub fn sync(&self) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            let result = writer
                .lock()
                .sync()
                .map_err(|e| StorageError::wal_error(format!("Failed to sync WAL: {:?}", e)));
            match result {
                Ok(()) => {
                    self.sync_count.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    self.sync_failures.fetch_add(1, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    pub fn append_redo<T: Serialize>(
        &self,
        op_type: WalOpType,
        timestamp: Timestamp,
        redo: &T,
    ) -> StorageResult<()> {
        let Some(writer) = self.local_writer.as_ref() else {
            return Err(StorageError::wal_error(
                "WAL writer is not initialized".to_string(),
            ));
        };

        let payload = to_allocvec(redo).map_err(|e| {
            StorageError::serialize_error(format!("Failed to serialize WAL redo: {}", e))
        })?;

        writer
            .lock()
            .append_entry(op_type, timestamp, &payload)
            .map_err(|e| StorageError::wal_error(format!("Failed to append WAL entry: {}", e)))?;

        Ok(())
    }

    pub fn append_entry(
        &self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> StorageResult<()> {
        let Some(writer) = self.local_writer.as_ref() else {
            return Err(StorageError::wal_error(
                "WAL writer is not initialized".to_string(),
            ));
        };
        writer
            .lock()
            .append_entry(op_type, timestamp, payload)
            .map_err(|e| StorageError::wal_error(format!("Failed to append WAL entry: {}", e)))
    }

    pub fn append_transaction_with_durability(
        &self,
        transaction_id: TransactionId,
        entries: Vec<TransactionWalEntry>,
        intents: &[OutboxIntent],
        durability: crate::core::types::DurabilityLevel,
    ) -> StorageResult<CommitLsn> {
        let Some(writer) = self.local_writer.as_ref() else {
            return Err(StorageError::wal_error(
                "WAL writer is not initialized".to_string(),
            ));
        };

        // Append the commit batch under the writer lock, then release it
        // before waiting for durability. For Sync durability this lets
        // concurrent commits batch into a single fsync through the
        // group-commit coordinator instead of serializing one fsync per
        // transaction at the writer lock.
        let lsn = {
            let mut guard = writer.lock();
            guard
                .append_transaction_batch_no_wait(transaction_id, entries, intents)
                .map_err(|error| {
                    StorageError::wal_error(format!(
                        "Failed to append committed WAL transaction: {}",
                        error
                    ))
                })?
        };

        // Record the append in the group-commit coordinator for every
        // durability level when group commit is enabled. For Sync this is the
        // waiting path below; for Async this lets the transaction's WAL bytes
        // be covered by the next leader's fsync (free durability piggyback,
        // amortizing fsyncs across Sync + Async writers).
        let coordinator = writer.lock().group_commit_coordinator().cloned();
        if let Some(ref coordinator) = coordinator {
            coordinator.record_appended(lsn.get());
        }

        if matches!(durability, crate::core::types::DurabilityLevel::Sync) {
            if let Some(ref coordinator) = coordinator {
                coordinator.append_and_wait(lsn.get()).map_err(|error| {
                    StorageError::wal_error(format!(
                        "Failed to await durable WAL transaction: {}",
                        error
                    ))
                })?;
            } else {
                writer.lock().wait_for_durable(lsn.get()).map_err(|error| {
                    StorageError::wal_error(format!(
                        "Failed to sync committed WAL transaction: {}",
                        error
                    ))
                })?;
            }
        }

        Ok(lsn)
    }

    pub fn set_checkpoint_seq(&self, seq: u64) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.lock().set_checkpoint_seq(seq).map_err(|e| {
                StorageError::wal_error(format!("Failed to update checkpoint seq: {:?}", e))
            })?;
        }
        Ok(())
    }

    pub fn set_current_lsn(&self, lsn: Lsn) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.lock().set_current_lsn(lsn);
        }
        Ok(())
    }

    /// Restore the logical WAL baseline covered by a durable checkpoint.
    pub fn set_recovery_baseline_lsn(&self, lsn: Lsn) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.lock().set_recovery_baseline_lsn(lsn).map_err(|e| {
                StorageError::wal_error(format!("Failed to restore WAL recovery baseline: {:?}", e))
            })?;
        }
        Ok(())
    }

    pub fn truncate(&self, lsn: Lsn) -> StorageResult<()> {
        let truncation_lsn = self
            .truncation_barrier_lsn()
            .map_or(lsn, |barrier| lsn.min(barrier));
        if let Some(ref writer) = self.local_writer {
            writer.lock().truncate(truncation_lsn).map_err(|e| {
                StorageError::wal_error(format!(
                    "Failed to truncate WAL at {} (requested {}): {:?}",
                    truncation_lsn, lsn, e
                ))
            })?;
        }
        Ok(())
    }
}

impl Default for WalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_wal_manager_open_and_current_lsn() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut manager = WalManager::new();

        manager
            .open(temp_dir.path(), 0)
            .expect("Failed to open WAL");

        assert_eq!(manager.current_lsn(), Lsn::ZERO);
    }

    #[test]
    fn test_wal_metrics_track_syncs() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut manager = WalManager::new();
        manager
            .open(temp_dir.path(), 0)
            .expect("Failed to open WAL");

        manager.sync().expect("WAL sync should succeed");
        let metrics = manager.metrics();
        assert_eq!(metrics.sync_count, 1);
        assert_eq!(metrics.sync_failures, 0);
        assert_eq!(metrics.accepted_lsn, manager.current_lsn());
        assert_eq!(metrics.durable_lsn, manager.durable_lsn());
    }

    #[test]
    fn truncation_barrier_uses_the_oldest_published_index() {
        use parking_lot::RwLock;
        let mut manager = WalManager::new();
        let registry = Arc::new(RwLock::new(std::collections::HashMap::from([
            ((1, 1), CommitLsn::new(80)),
            ((1, 2), CommitLsn::new(120)),
        ])));
        manager.set_index_barrier_registry(registry);

        assert_eq!(
            manager.truncation_barrier_lsn(),
            Some(Lsn::new(80)),
            "WAL truncation must stop at the oldest active index barrier"
        );
    }

    #[test]
    fn test_async_append_records_coordinator_for_piggyback_sync() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut manager = WalManager::new();
        manager
            .open(temp_dir.path(), 0)
            .expect("Failed to open WAL");

        // Async append: recorded in the coordinator but must not block on fsync.
        let lsn = manager
            .append_transaction_with_durability(
                crate::core::types::TransactionId::new(1),
                Vec::new(),
                &[],
                crate::core::types::DurabilityLevel::Async,
            )
            .expect("async append must not wait");
        assert!(lsn.get() > 0, "async append must still advance the WAL LSN");

        // A later explicit sync must make the async append durable: the
        // coordinator now knows about the appended LSN, so the sync's fsync
        // covers it (this is the free-durability piggyback path).
        manager.sync().expect("sync should succeed");
        assert!(
            manager.durable_lsn().as_u64() >= lsn.get(),
            "sync after async append must cover the appended LSN"
        );
    }

    #[test]
    fn test_none_durability_does_not_block() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let mut manager = WalManager::new();
        manager
            .open(temp_dir.path(), 0)
            .expect("Failed to open WAL");

        let lsn = manager
            .append_transaction_with_durability(
                crate::core::types::TransactionId::new(2),
                Vec::new(),
                &[],
                crate::core::types::DurabilityLevel::None,
            )
            .expect("none durability append must not wait");
        assert!(lsn.get() > 0);
    }
}

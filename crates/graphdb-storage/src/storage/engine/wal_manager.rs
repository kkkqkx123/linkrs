//! WAL Manager
//!
//! Unified WAL (Write-Ahead Log) manager that properly integrates with LocalWalWriter.
//! This module provides a single source of truth for LSN management and WAL operations.

use crate::core::types::{CommitLsn, TransactionId};
use crate::core::wal::types::WalOpType;
use crate::core::wal::OutboxIntent;
use crate::core::{StorageError, StorageResult};
use crate::storage::index::shard_runtime::IndexBarrierRegistry;
use crate::transaction::wal::writer::WalWriter;
use crate::transaction::wal::TransactionWalEntry;
use crate::transaction::wal::{LocalWalWriter, Lsn, WalConfig};
use parking_lot::RwLock;
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
    local_writer: Option<Arc<RwLock<LocalWalWriter>>>,
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
        self.local_writer = Some(Arc::new(RwLock::new(writer)));
        Ok(())
    }

    pub fn writer(&self) -> Option<Arc<RwLock<LocalWalWriter>>> {
        self.local_writer.clone()
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
            writer.read().current_lsn()
        } else {
            Lsn::ZERO
        }
    }

    /// Return the latest WAL position confirmed durable by the writer.
    pub fn durable_lsn(&self) -> Lsn {
        if let Some(ref writer) = self.local_writer {
            writer.read().durable_lsn()
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
                .write()
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
        timestamp: u32,
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
            .write()
            .append_entry(op_type, timestamp, &payload)
            .map_err(|e| StorageError::wal_error(format!("Failed to append WAL entry: {:?}", e)))?;

        Ok(())
    }

    pub fn append_transaction(
        &self,
        transaction_id: TransactionId,
        entries: Vec<TransactionWalEntry>,
        intents: &[OutboxIntent],
    ) -> StorageResult<CommitLsn> {
        self.append_transaction_with_durability(
            transaction_id,
            entries,
            intents,
            crate::core::types::DurabilityLevel::Sync,
        )
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
        writer
            .write()
            .append_transaction_batch_with_durability(transaction_id, entries, intents, durability)
            .map_err(|error| {
                StorageError::wal_error(format!(
                    "Failed to append committed WAL transaction: {}",
                    error
                ))
            })
    }

    pub fn set_checkpoint_seq(&self, seq: u64) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.write().set_checkpoint_seq(seq).map_err(|e| {
                StorageError::wal_error(format!("Failed to update checkpoint seq: {:?}", e))
            })?;
        }
        Ok(())
    }

    pub fn set_current_lsn(&self, lsn: Lsn) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.write().set_current_lsn(lsn);
        }
        Ok(())
    }

    /// Restore the logical WAL baseline covered by a durable checkpoint.
    pub fn set_recovery_baseline_lsn(&self, lsn: Lsn) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer.write().set_recovery_baseline_lsn(lsn).map_err(|e| {
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
            writer.write().truncate(truncation_lsn).map_err(|e| {
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

        assert!(manager.writer().is_some());
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
}

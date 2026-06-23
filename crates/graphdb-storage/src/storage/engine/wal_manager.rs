//! WAL Manager
//!
//! Unified WAL (Write-Ahead Log) manager that properly integrates with LocalWalWriter.
//! This module provides a single source of truth for LSN management and WAL operations.

use crate::core::{StorageError, StorageResult};
use crate::core::wal::types::WalOpType;
use crate::transaction::wal::writer::WalWriter;
use crate::transaction::wal::{LocalWalWriter, Lsn, WalConfig};
use parking_lot::RwLock;
use postcard::to_allocvec;
use serde::Serialize;
use std::path::Path;
use std::sync::Arc;

/// Unified WAL manager that wraps LocalWalWriter
///
/// This manager ensures LSN consistency by delegating all LSN operations
/// to the underlying LocalWalWriter, avoiding the dual LSN tracking issue.
pub struct WalManager {
    local_writer: Option<Arc<RwLock<LocalWalWriter>>>,
    config: WalConfig,
}

impl WalManager {
    pub fn new() -> Self {
        Self {
            local_writer: None,
            config: WalConfig::default(),
        }
    }

    pub fn with_config(config: WalConfig) -> Self {
        Self {
            local_writer: None,
            config,
        }
    }

    pub fn open(&mut self, wal_dir: &Path, thread_id: u32) -> StorageResult<()> {
        let wal_uri = wal_dir.to_string_lossy().to_string();
        let mut writer = LocalWalWriter::with_config(&wal_uri, thread_id, self.config.clone());
        writer
            .open()
            .map_err(|e| StorageError::wal_error(format!("Failed to open WAL: {:?}", e)))?;
        self.local_writer = Some(Arc::new(RwLock::new(writer)));
        Ok(())
    }

    pub fn writer(&self) -> Option<Arc<RwLock<LocalWalWriter>>> {
        self.local_writer.clone()
    }

    pub fn current_lsn(&self) -> Lsn {
        if let Some(ref writer) = self.local_writer {
            writer.read().current_lsn()
        } else {
            Lsn::ZERO
        }
    }

    pub fn sync(&self) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer
                .write()
                .sync()
                .map_err(|e| StorageError::wal_error(format!("Failed to sync WAL: {:?}", e)))?;
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

    pub fn truncate(&self, lsn: Lsn) -> StorageResult<()> {
        if let Some(ref writer) = self.local_writer {
            writer
                .write()
                .truncate(lsn)
                .map_err(|e| StorageError::wal_error(format!("Failed to truncate WAL: {:?}", e)))?;
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
}

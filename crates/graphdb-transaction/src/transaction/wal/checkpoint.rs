//! Unified Checkpoint Manager
//!
//! Provides checkpoint functionality for faster recovery, WAL file management,
//! and table modification tracking.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::wal::types::{Lsn, TransactionId, WalError, WalFileHeader, WalResult, WAL_FILE_HEADER_SIZE};
use crate::core::types::Timestamp;
use crate::core::types::{TableId, TableTracker};

/// Checkpoint information
#[derive(Debug, Clone)]
pub struct Checkpoint {
    /// Checkpoint sequence number
    pub seq: u64,
    /// Timestamp of the checkpoint
    pub timestamp: Timestamp,
    /// LSN (Log Sequence Number) at checkpoint
    pub lsn: Lsn,
    /// WAL files that can be safely deleted after this checkpoint
    pub wal_files: Vec<PathBuf>,
    /// Active transactions at checkpoint time
    pub active_transactions: Vec<TransactionId>,
    /// Redo LSN (where recovery should start)
    pub redo_lsn: Lsn,
}

/// Checkpoint mode (similar to SQLite's checkpoint modes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckpointMode {
    /// Passive: checkpoint as many frames as possible without blocking writers
    Passive,
    /// Full: block until no writers, checkpoint all frames
    #[default]
    Full,
    /// Restart: same as Full, but ensures next writer restarts log
    Restart,
    /// Truncate: same as Restart, but also truncates WAL file
    Truncate,
}

/// Result of a checkpoint operation
#[derive(Debug, Clone, Default)]
pub struct CheckpointResult {
    /// Number of pages checkpointed
    pub pages_written: usize,
    /// Number of WAL files processed
    pub wal_files_processed: usize,
    /// Duration of checkpoint in microseconds
    pub duration_us: u64,
    /// Checkpoint mode used
    pub mode: CheckpointMode,
    /// Whether the checkpoint was successful
    pub success: bool,
}

/// Checkpoint manager
pub struct CheckpointManager {
    /// WAL directory path
    wal_dir: PathBuf,
    /// Current checkpoint sequence
    current_seq: u64,
    /// Last checkpoint timestamp
    last_checkpoint_ts: Timestamp,
    /// Last checkpoint LSN
    last_checkpoint_lsn: Lsn,
    /// Checkpoint file path
    checkpoint_file: PathBuf,
    /// Active transactions
    active_transactions: Vec<TransactionId>,
    /// Modified tables
    modified_tables: Vec<TableId>,
    /// Table tracker for modification tracking
    table_tracker: Option<Arc<TableTracker>>,
    /// Checkpoint count
    checkpoint_count: AtomicU64,
    /// Work directory for checkpoint metadata
    work_dir: PathBuf,
}

impl CheckpointManager {
    /// Create a new checkpoint manager with optional table tracker
    pub fn new(wal_dir: &Path, work_dir: &Path, table_tracker: Option<Arc<TableTracker>>) -> Self {
        let checkpoint_file = wal_dir.join("checkpoint.meta");
        Self {
            wal_dir: wal_dir.to_path_buf(),
            work_dir: work_dir.to_path_buf(),
            current_seq: 0,
            last_checkpoint_ts: 0,
            last_checkpoint_lsn: Lsn::ZERO,
            checkpoint_file,
            active_transactions: Vec::new(),
            modified_tables: Vec::new(),
            table_tracker,
            checkpoint_count: AtomicU64::new(0),
        }
    }

    /// Initialize checkpoint manager and load existing checkpoint info
    pub fn init(&mut self) -> WalResult<()> {
        if !self.wal_dir.exists() {
            fs::create_dir_all(&self.wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;
        }

        if !self.work_dir.exists() {
            fs::create_dir_all(&self.work_dir).map_err(|e| WalError::IoError(e.to_string()))?;
        }

        self.load_checkpoint_meta()?;
        Ok(())
    }

    /// Load checkpoint metadata from file
    fn load_checkpoint_meta(&mut self) -> WalResult<()> {
        if !self.checkpoint_file.exists() {
            return Ok(());
        }

        let content = fs::read_to_string(&self.checkpoint_file)
            .map_err(|e| WalError::IoError(e.to_string()))?;

        for line in content.lines() {
            if let Some((key, value)) = line.split_once('=') {
                match key.trim() {
                    "seq" => {
                        self.current_seq = value.trim().parse().unwrap_or(0);
                    }
                    "timestamp" => {
                        self.last_checkpoint_ts = value.trim().parse().unwrap_or(0);
                    }
                    "lsn" => {
                        self.last_checkpoint_lsn = Lsn::new(value.trim().parse().unwrap_or(0));
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Save checkpoint metadata to file
    fn save_checkpoint_meta(&self) -> WalResult<()> {
        let content = format!(
            "seq={}\ntimestamp={}\nlsn={}\n",
            self.current_seq,
            self.last_checkpoint_ts,
            self.last_checkpoint_lsn.as_u64()
        );

        fs::write(&self.checkpoint_file, content).map_err(|e| WalError::IoError(e.to_string()))?;

        Ok(())
    }

    /// Register an active transaction
    pub fn register_transaction(&mut self, tx_id: TransactionId) {
        if !self.active_transactions.contains(&tx_id) {
            self.active_transactions.push(tx_id);
        }
    }

    /// Unregister a completed transaction
    pub fn unregister_transaction(&mut self, tx_id: TransactionId) {
        self.active_transactions.retain(|&id| id != tx_id);
    }

    /// Mark a table as modified
    pub fn mark_table_modified(&mut self, table_id: TableId) {
        if !self.modified_tables.contains(&table_id) {
            self.modified_tables.push(table_id);
        }
    }

    /// Mark a table as clean
    pub fn mark_table_clean(&mut self, table_id: TableId) {
        self.modified_tables.retain(|&id| id != table_id);
    }

    /// Get active transactions
    pub fn active_transactions(&self) -> &[TransactionId] {
        &self.active_transactions
    }

    /// Get modified tables
    pub fn modified_tables(&self) -> &[TableId] {
        &self.modified_tables
    }

    /// Create a new checkpoint
    pub fn create_checkpoint(&mut self, timestamp: Timestamp, lsn: Lsn) -> WalResult<Checkpoint> {
        self.current_seq += 1;
        self.last_checkpoint_ts = timestamp;
        self.last_checkpoint_lsn = lsn;

        let wal_files = self.get_wal_files_before_checkpoint()?;

        let redo_lsn = self.calculate_redo_lsn();

        let checkpoint = Checkpoint {
            seq: self.current_seq,
            timestamp,
            lsn,
            wal_files,
            active_transactions: self.active_transactions.clone(),
            redo_lsn,
        };

        self.save_checkpoint_meta()?;

        Ok(checkpoint)
    }

    /// Calculate the redo LSN (where recovery should start)
    fn calculate_redo_lsn(&self) -> Lsn {
        if self.active_transactions.is_empty() {
            self.last_checkpoint_lsn
        } else {
            Lsn::ZERO
        }
    }

    /// Create a checkpoint with modified tables
    pub fn create_checkpoint_with_tables(
        &mut self,
        timestamp: Timestamp,
        lsn: Lsn,
        modified_tables: Vec<TableId>,
    ) -> WalResult<Checkpoint> {
        self.modified_tables = modified_tables;
        self.create_checkpoint(timestamp, lsn)
    }

    /// Create a checkpoint with specified mode
    pub fn checkpoint(
        &mut self,
        timestamp: Timestamp,
        lsn: Lsn,
        mode: CheckpointMode,
    ) -> WalResult<CheckpointResult> {
        let start_time = std::time::Instant::now();

        let checkpoint = match mode {
            CheckpointMode::Passive => self.checkpoint_passive(timestamp, lsn)?,
            CheckpointMode::Full => self.create_checkpoint(timestamp, lsn)?,
            CheckpointMode::Restart => self.checkpoint_restart(timestamp, lsn)?,
            CheckpointMode::Truncate => self.checkpoint_truncate(timestamp, lsn)?,
        };

        let duration_us = start_time.elapsed().as_micros() as u64;

        Ok(CheckpointResult {
            pages_written: self.modified_tables.len(),
            wal_files_processed: checkpoint.wal_files.len(),
            duration_us,
            mode,
            success: true,
        })
    }

    /// Passive checkpoint: checkpoint without blocking writers
    fn checkpoint_passive(&mut self, timestamp: Timestamp, lsn: Lsn) -> WalResult<Checkpoint> {
        self.create_checkpoint(timestamp, lsn)
    }

    /// Restart checkpoint: full checkpoint and reset log
    fn checkpoint_restart(&mut self, timestamp: Timestamp, lsn: Lsn) -> WalResult<Checkpoint> {
        let checkpoint = self.create_checkpoint(timestamp, lsn)?;
        self.modified_tables.clear();
        Ok(checkpoint)
    }

    /// Truncate checkpoint: full checkpoint, reset log, and truncate WAL
    fn checkpoint_truncate(&mut self, timestamp: Timestamp, lsn: Lsn) -> WalResult<Checkpoint> {
        let checkpoint = self.checkpoint_restart(timestamp, lsn)?;

        for wal_file in &checkpoint.wal_files {
            if wal_file.exists() {
                fs::remove_file(wal_file).map_err(|e| WalError::IoError(e.to_string()))?;
            }
        }

        Ok(checkpoint)
    }

    /// Get WAL files that can be deleted before current checkpoint
    fn get_wal_files_before_checkpoint(&self) -> WalResult<Vec<PathBuf>> {
        let mut wal_files = Vec::new();

        if !self.wal_dir.exists() {
            return Ok(wal_files);
        }

        let entries = fs::read_dir(&self.wal_dir).map_err(|e| WalError::IoError(e.to_string()))?;

        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "wal")
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("thread_") && n.contains("_wal_"))
            {
                if let Ok(true) = self.is_wal_file_before_checkpoint(&path) {
                    wal_files.push(path);
                }
            }
        }

        Ok(wal_files)
    }

    /// Check if a WAL file is before the current checkpoint
    fn is_wal_file_before_checkpoint(&self, path: &Path) -> WalResult<bool> {
        use std::fs::File;
        use std::io::Read;

        let mut file = File::open(path).map_err(|e| WalError::IoError(e.to_string()))?;

        let mut buffer = [0u8; WAL_FILE_HEADER_SIZE];
        if let Ok(()) = file.read_exact(&mut buffer) {
            if let Some(header) = WalFileHeader::from_bytes(&buffer) {
                return Ok(header.checkpoint_seq < self.current_seq);
            }
        }

        Ok(false)
    }

    /// Clean up WAL files before a checkpoint
    pub fn cleanup_before_checkpoint(&self, checkpoint: &Checkpoint) -> WalResult<usize> {
        let mut deleted_count = 0;

        for path in &checkpoint.wal_files {
            if path.exists() {
                fs::remove_file(path).map_err(|e| WalError::IoError(e.to_string()))?;
                deleted_count += 1;
            }
        }

        Ok(deleted_count)
    }

    /// Get current checkpoint sequence
    pub fn current_seq(&self) -> u64 {
        self.current_seq
    }

    /// Get last checkpoint timestamp
    pub fn last_checkpoint_ts(&self) -> Timestamp {
        self.last_checkpoint_ts
    }

    /// Get last checkpoint LSN
    pub fn last_checkpoint_lsn(&self) -> Lsn {
        self.last_checkpoint_lsn
    }

    /// Get the latest checkpoint info
    pub fn get_latest_checkpoint(&self) -> Option<Checkpoint> {
        if self.current_seq == 0 {
            return None;
        }

        Some(Checkpoint {
            seq: self.current_seq,
            timestamp: self.last_checkpoint_ts,
            lsn: self.last_checkpoint_lsn,
            wal_files: Vec::new(),
            active_transactions: self.active_transactions.clone(),
            redo_lsn: self.calculate_redo_lsn(),
        })
    }

    /// Create checkpoint using table tracker
    pub fn create_checkpoint_with_tracker(
        &mut self,
        timestamp: Timestamp,
        lsn: Lsn,
        mode: CheckpointMode,
    ) -> WalResult<CheckpointResult> {
        let modified_tables: Vec<TableId> = if let Some(ref tracker) = self.table_tracker {
            tracker.get_modified_tables()
        } else {
            self.modified_tables.clone()
        };

        self.modified_tables = modified_tables;

        let result = self.checkpoint(timestamp, lsn, mode)?;

        if mode == CheckpointMode::Restart || mode == CheckpointMode::Truncate {
            if let Some(ref tracker) = self.table_tracker {
                tracker.clear();
            }
        }

        self.checkpoint_count.fetch_add(1, Ordering::Relaxed);

        Ok(result)
    }

    /// Get checkpoint count
    pub fn checkpoint_count(&self) -> u64 {
        self.checkpoint_count.load(Ordering::Relaxed)
    }

    /// Get table tracker reference
    pub fn table_tracker(&self) -> Option<&Arc<TableTracker>> {
        self.table_tracker.as_ref()
    }

    /// Sync modified tables from tracker
    pub fn sync_modified_tables_from_tracker(&mut self) {
        if let Some(ref tracker) = self.table_tracker {
            let tracked_tables = tracker.get_modified_tables();
            for table_id in tracked_tables {
                if !self.modified_tables.contains(&table_id) {
                    self.modified_tables.push(table_id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_checkpoint_manager() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        assert_eq!(manager.current_seq(), 0);
        assert_eq!(manager.last_checkpoint_ts(), 0);
        assert_eq!(manager.last_checkpoint_lsn(), Lsn::ZERO);
    }

    #[test]
    fn test_create_checkpoint() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        let checkpoint = manager
            .create_checkpoint(100, Lsn::new(1000))
            .expect("Failed to create checkpoint");

        assert_eq!(checkpoint.seq, 1);
        assert_eq!(checkpoint.timestamp, 100);
        assert_eq!(checkpoint.lsn, Lsn::new(1000));
        assert_eq!(manager.current_seq(), 1);
        assert_eq!(manager.last_checkpoint_ts(), 100);
        assert_eq!(manager.last_checkpoint_lsn(), Lsn::new(1000));
    }

    #[test]
    fn test_checkpoint_persistence() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        {
            let mut manager = CheckpointManager::new(wal_path, work_path, None);
            manager.init().expect("Failed to init");
            manager
                .create_checkpoint(100, Lsn::new(1000))
                .expect("Failed to create checkpoint");
        }

        {
            let mut manager = CheckpointManager::new(wal_path, work_path, None);
            manager.init().expect("Failed to init");
            assert_eq!(manager.current_seq(), 1);
            assert_eq!(manager.last_checkpoint_ts(), 100);
            assert_eq!(manager.last_checkpoint_lsn(), Lsn::new(1000));
        }
    }

    #[test]
    fn test_get_latest_checkpoint() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        assert!(manager.get_latest_checkpoint().is_none());

        manager
            .create_checkpoint(100, Lsn::new(1000))
            .expect("Failed to create checkpoint");

        let latest = manager.get_latest_checkpoint().expect("No checkpoint");
        assert_eq!(latest.seq, 1);
        assert_eq!(latest.timestamp, 100);
        assert_eq!(latest.lsn, Lsn::new(1000));
    }

    #[test]
    fn test_active_transactions() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        manager.register_transaction(TransactionId(1));
        manager.register_transaction(TransactionId(2));
        manager.register_transaction(TransactionId(1));

        assert_eq!(manager.active_transactions().len(), 2);
        assert!(manager.active_transactions().contains(&TransactionId(1)));
        assert!(manager.active_transactions().contains(&TransactionId(2)));

        manager.unregister_transaction(TransactionId(1));
        assert_eq!(manager.active_transactions().len(), 1);
        assert!(!manager.active_transactions().contains(&TransactionId(1)));
    }

    #[test]
    fn test_modified_tables() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        manager.mark_table_modified(TableId::vertex(1));
        manager.mark_table_modified(TableId::vertex(2));
        manager.mark_table_modified(TableId::vertex(1));

        assert_eq!(manager.modified_tables().len(), 2);

        manager.mark_table_clean(TableId::vertex(1));
        assert_eq!(manager.modified_tables().len(), 1);
    }

    #[test]
    fn test_checkpoint_with_tables() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let mut manager = CheckpointManager::new(wal_path, work_path, None);
        manager.init().expect("Failed to init");

        let modified_tables = vec![TableId::vertex(1), TableId::edge(2)];
        let _checkpoint = manager
            .create_checkpoint_with_tables(100, Lsn::new(1000), modified_tables.clone())
            .expect("Failed to create checkpoint");

        assert_eq!(manager.modified_tables().len(), 2);
    }

    #[test]
    fn test_checkpoint_with_table_tracker() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let wal_path = temp_dir.path();
        let work_path = temp_dir.path();

        let table_tracker = Arc::new(TableTracker::new(1000, std::time::Duration::from_secs(60)));
        let mut manager = CheckpointManager::new(wal_path, work_path, Some(table_tracker.clone()));
        manager.init().expect("Failed to init");

        table_tracker.mark_modified(TableId::vertex(1));

        manager.sync_modified_tables_from_tracker();
        assert_eq!(manager.modified_tables().len(), 1);

        let result = manager
            .create_checkpoint_with_tracker(100, Lsn::new(1000), CheckpointMode::Full)
            .expect("Failed to create checkpoint");

        assert!(result.success);
        assert_eq!(manager.checkpoint_count(), 1);
    }
}

//! Persistence Coordinator
//!
//! Unified coordinator for the persistence responsibility chain:
//!
//! ```text
//! Write Operations
//!     ↓
//! WAL (Write-Ahead Log) - Guarantees durability
//!     ↓
//! Memory (RAM) - Provides fast access
//!     ↓
//! Flush (Periodic) - Writes memory data to disk
//!     ↓
//! Checkpoint (Periodic) - Creates consistent snapshots
//!     ↓
//! Snapshot (Manual) - User-triggered full backup
//! ```
//!
//! Responsibilities:
//! - WalManager: WAL log management, ensures write-ahead logging
//! - PropertyGraph::flush_to_disk(): Memory-to-disk flushing (triggered by coordinator)
//! - CheckpointManager: Checkpoint creation and recovery
//! - SnapshotManager: Full backup management
//!
//! Usage:
//! 1. Write operations go through WAL first
//! 2. Periodic flush is triggered by the coordinator based on thresholds
//! 3. Checkpoints are created periodically or on demand
//! 4. Snapshots are user-triggered for full backups

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;

use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::storage::engine::snapshot_manager::{SnapshotManager, SnapshotOptions};
use crate::storage::engine::WalManager;
use crate::transaction::wal::{CheckpointManager, Lsn, SyncPolicy, WalConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
    Idle,
    Checkpointing,
    Snapshotting,
}

#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    pub checkpoint_id: u64,
    pub lsn: Lsn,
    pub timestamp: u32,
}

#[derive(Debug, Clone)]
pub struct CheckpointStats {
    pub checkpoint_id: u64,
    pub data_flushed: u64,
    pub wal_truncated: u64,
    pub duration: Duration,
    pub snapshot_created: bool,
}

#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    pub data_dir: PathBuf,
    pub wal_dir: PathBuf,
    pub checkpoint_dir: PathBuf,
    pub snapshot_dir: PathBuf,
    pub auto_flush_interval: Duration,
    pub auto_checkpoint_interval: Duration,
    pub checkpoint_threshold: u64,
    pub max_wal_size: u64,
    pub enable_snapshots: bool,
    pub snapshot_interval: Duration,
    /// Should WAL be enabled
    pub enable_wal: bool,
    /// Synchronization policy for WAL write-ahead logging
    pub sync_policy: Option<SyncPolicy>,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            wal_dir: PathBuf::from("wal"),
            checkpoint_dir: PathBuf::from("checkpoint"),
            snapshot_dir: PathBuf::from("snapshots"),
            auto_flush_interval: Duration::from_secs(60),
            auto_checkpoint_interval: Duration::from_secs(300),
            checkpoint_threshold: 10000,
            max_wal_size: 100 * 1024 * 1024,
            enable_snapshots: true,
            snapshot_interval: Duration::from_secs(3600),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
        }
    }
}

impl PersistenceConfig {
    pub fn for_work_dir(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        Self {
            data_dir: path.join("data"),
            wal_dir: path.join("wal"),
            checkpoint_dir: path.join("checkpoint"),
            snapshot_dir: path.join("snapshots"),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
            ..Default::default()
        }
    }
}

pub struct PersistenceCoordinator {
    config: PersistenceConfig,
    wal_manager: Option<Arc<RwLock<WalManager>>>,
    checkpoint_manager: RwLock<CheckpointManager>,
    snapshot_manager: Option<Arc<SnapshotManager>>,
    last_checkpoint_time: RwLock<Instant>,
    last_flush_time: RwLock<Instant>,
    last_checkpoint_lsn: RwLock<Lsn>,
    last_flush_lsn: RwLock<Lsn>,
    last_snapshot_time: RwLock<Option<SystemTime>>,
    state: RwLock<PersistenceState>,
}

impl PersistenceCoordinator {
    pub fn new(config: PersistenceConfig) -> StorageResult<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        std::fs::create_dir_all(&config.checkpoint_dir)?;

        let wal_manager = if config.enable_wal {
            std::fs::create_dir_all(&config.wal_dir)?;
            let mut wal_cfg = WalConfig::default();
            if let Some(ref sp) = config.sync_policy {
                wal_cfg.sync_policy = *sp;
            }
            let mut wal_manager = WalManager::with_config(wal_cfg);
            wal_manager.open(&config.wal_dir, 0)?;
            Some(Arc::new(RwLock::new(wal_manager)))
        } else {
            None
        };

        let mut checkpoint_manager =
            CheckpointManager::new(&config.wal_dir, &config.checkpoint_dir, None);
        checkpoint_manager.init().map_err(|e| {
            crate::core::StorageError::db_error(format!("Failed to init checkpoint manager: {}", e))
        })?;

        if let Some(ref wal) = wal_manager {
            wal.read()
                .set_checkpoint_seq(checkpoint_manager.current_seq())?;
        }

        let snapshot_manager = if config.enable_snapshots {
            std::fs::create_dir_all(&config.snapshot_dir)?;
            Some(Arc::new(SnapshotManager::new(
                config.snapshot_dir.clone(),
                config.data_dir.join("snapshot_work"),
            )?))
        } else {
            None
        };

        Ok(Self {
            config,
            wal_manager,
            checkpoint_manager: RwLock::new(checkpoint_manager),
            snapshot_manager,
            last_checkpoint_time: RwLock::new(Instant::now()),
            last_flush_time: RwLock::new(Instant::now()),
            last_checkpoint_lsn: RwLock::new(Lsn::ZERO),
            last_flush_lsn: RwLock::new(Lsn::ZERO),
            last_snapshot_time: RwLock::new(None),
            state: RwLock::new(PersistenceState::Idle),
        })
    }

    pub fn wal_manager(&self) -> Option<Arc<RwLock<WalManager>>> {
        self.wal_manager.clone()
    }

    pub fn wal_dir(&self) -> PathBuf {
        self.config.wal_dir.clone()
    }

    pub fn checkpoint_dir(&self) -> PathBuf {
        self.config.checkpoint_dir.clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        self.config.data_dir.clone()
    }

    fn set_state(&self, state: PersistenceState) {
        *self.state.write() = state;
    }

    fn current_lsn(&self) -> Lsn {
        match &self.wal_manager {
            Some(wal) => wal.read().current_lsn(),
            None => Lsn::ZERO,
        }
    }

    fn wal_bytes_since(&self, base_lsn: Lsn) -> u64 {
        self.current_lsn().offset_in_file(base_lsn)
    }

    pub fn should_flush(&self) -> bool {
        let last_flush_lsn = *self.last_flush_lsn.read();
        let last_flush = *self.last_flush_time.read();

        self.wal_bytes_since(last_flush_lsn) >= self.config.checkpoint_threshold
            || last_flush.elapsed() >= self.config.auto_flush_interval
    }

    pub fn should_checkpoint(&self) -> bool {
        let last_checkpoint_lsn = *self.last_checkpoint_lsn.read();
        let last_checkpoint = *self.last_checkpoint_time.read();
        let wal_bytes_since_checkpoint = self.wal_bytes_since(last_checkpoint_lsn);

        wal_bytes_since_checkpoint >= self.config.checkpoint_threshold
            || wal_bytes_since_checkpoint >= self.config.max_wal_size
            || last_checkpoint.elapsed() >= self.config.auto_checkpoint_interval
    }

    pub fn should_snapshot(&self) -> bool {
        if !self.config.enable_snapshots {
            return false;
        }

        if let Some(last_snapshot) = *self.last_snapshot_time.read() {
            if let Ok(elapsed) = last_snapshot.elapsed() {
                return elapsed >= self.config.snapshot_interval;
            }
        }

        true
    }

    pub fn create_checkpoint(
        &self,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
    ) -> StorageResult<CheckpointStats> {
        let start = Instant::now();

        self.set_state(PersistenceState::Checkpointing);

        let wal_lsn = {
            match &self.wal_manager {
                Some(wal) => wal.read().current_lsn(),
                None => Lsn::ZERO,
            }
        };

        log::info!(
            "Creating checkpoint at timestamp {}, LSN {}",
            timestamp,
            wal_lsn
        );

        let checkpoint = {
            let mut cm = self.checkpoint_manager.write();
            cm.create_checkpoint(timestamp, wal_lsn).map_err(|e| {
                crate::core::StorageError::db_error(format!("Failed to create checkpoint: {}", e))
            })?
        };

        if let Some(ref wal) = self.wal_manager {
            wal.read().set_checkpoint_seq(checkpoint.seq)?;
        }

        let checkpoint_dir = self
            .config
            .checkpoint_dir
            .join(format!("checkpoint_{}", checkpoint.seq));
        std::fs::create_dir_all(&checkpoint_dir)?;

        let data = flush_data(&checkpoint_dir, timestamp)?;

        self.save_checkpoint_metadata(&checkpoint_dir, &checkpoint, &data)?;

        if let Some(ref wal) = self.wal_manager {
            wal.read().truncate(wal_lsn)?;
        }

        self.mark_checkpointed(wal_lsn);

        let snapshot_created = if self.should_snapshot() {
            self.set_state(PersistenceState::Snapshotting);
            if let Some(ref snapshot_manager) = self.snapshot_manager {
                let snapshot_options = SnapshotOptions::default();
                match snapshot_manager.create_snapshot(
                    crate::storage::engine::snapshot_manager::CreateSnapshotParams {
                        data_dir: self.config.data_dir.clone(),
                        snapshot_id: checkpoint.seq,
                        vertex_count: data.vertex_count,
                        edge_count: data.edge_count,
                        checkpoint_seq: checkpoint.seq,
                        wal_lsn: wal_lsn.into(),
                        options: snapshot_options,
                    },
                ) {
                    Ok(_) => {
                        *self.last_snapshot_time.write() = Some(SystemTime::now());
                        true
                    }
                    Err(e) => {
                        log::error!("Failed to create snapshot: {}", e);
                        false
                    }
                }
            } else {
                false
            }
        } else {
            false
        };

        self.set_state(PersistenceState::Idle);

        let stats = CheckpointStats {
            checkpoint_id: checkpoint.seq,
            data_flushed: data.data_size,
            wal_truncated: wal_lsn.into(),
            duration: start.elapsed(),
            snapshot_created,
        };

        log::info!(
            "Checkpoint {} completed in {:?}",
            checkpoint.seq,
            stats.duration
        );

        Ok(stats)
    }

    fn save_checkpoint_metadata(
        &self,
        dir: &Path,
        checkpoint: &crate::transaction::wal::Checkpoint,
        data: &CheckpointData,
    ) -> StorageResult<()> {
        use std::fs::File;
        use std::io::Write;

        let metadata_path = dir.join("checkpoint.meta");
        let mut file = File::create(metadata_path)?;

        writeln!(file, "checkpoint_id={}", checkpoint.seq)?;
        writeln!(file, "timestamp={}", checkpoint.timestamp)?;
        writeln!(file, "wal_lsn={}", checkpoint.lsn.0)?;
        writeln!(file, "vertex_count={}", data.vertex_count)?;
        writeln!(file, "edge_count={}", data.edge_count)?;
        writeln!(file, "data_size={}", data.data_size)?;
        writeln!(file, "created_at={:?}", SystemTime::now())?;

        Ok(())
    }

    pub fn load_latest_checkpoint(
        &self,
        load_data: impl FnOnce(&Path) -> StorageResult<()>,
    ) -> StorageResult<Option<CheckpointInfo>> {
        let checkpoints_dir = &self.config.checkpoint_dir;

        if !checkpoints_dir.exists() {
            return Ok(None);
        }

        let mut checkpoints: Vec<(u64, PathBuf)> = std::fs::read_dir(checkpoints_dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name()?.to_string_lossy();
                    if name.starts_with("checkpoint_") {
                        let id: u64 = name.trim_start_matches("checkpoint_").parse().ok()?;
                        Some((id, path))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        checkpoints.sort_by_key(|(id, _)| std::cmp::Reverse(*id));

        if let Some((_, checkpoint_path)) = checkpoints.first() {
            let info = self.load_checkpoint_metadata(checkpoint_path)?;

            load_data(checkpoint_path)?;

            if let Some(ref wal) = self.wal_manager {
                wal.read().set_checkpoint_seq(info.checkpoint_id)?;
                wal.read().truncate(info.lsn)?;
            }

            return Ok(Some(info));
        }

        Ok(None)
    }

    fn load_checkpoint_metadata(&self, dir: &Path) -> StorageResult<CheckpointInfo> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let metadata_path = dir.join("checkpoint.meta");
        let file = File::open(metadata_path)?;
        let reader = BufReader::new(file);

        let mut checkpoint_id: Option<u64> = None;
        let mut lsn: Option<u64> = None;
        let mut timestamp: Option<u32> = None;

        for line in reader.lines() {
            let line = line?;
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(StorageError::deserialize_error(format!(
                    "Invalid checkpoint metadata line: {}",
                    line
                )));
            }

            match parts[0] {
                "checkpoint_id" => {
                    checkpoint_id = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid checkpoint_id in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "wal_lsn" => {
                    lsn = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid wal_lsn in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                "timestamp" => {
                    timestamp = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid timestamp in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
                _ => {}
            }
        }

        let checkpoint_id = checkpoint_id.ok_or_else(|| {
            StorageError::deserialize_error(
                "Missing checkpoint_id in checkpoint metadata".to_string(),
            )
        })?;
        let lsn = lsn.ok_or_else(|| {
            StorageError::deserialize_error("Missing wal_lsn in checkpoint metadata".to_string())
        })?;

        Ok(CheckpointInfo {
            checkpoint_id,
            lsn: Lsn::new(lsn),
            timestamp: timestamp.unwrap_or(0),
        })
    }

    pub fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        let snapshot_manager = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| StorageError::not_supported("Snapshots are not enabled"))?;

        snapshot_manager.verify_snapshot(snapshot_id)
    }

    pub fn cleanup_old_snapshots(&self) -> StorageResult<usize> {
        let snapshot_manager = self
            .snapshot_manager
            .as_ref()
            .ok_or_else(|| StorageError::not_supported("Snapshots are not enabled"))?;

        snapshot_manager.cleanup_old_snapshots()
    }

    pub fn snapshot_stats(&self) -> SnapshotStats {
        if let Some(snapshot_manager) = self.snapshot_manager.as_ref() {
            SnapshotStats {
                snapshot_count: snapshot_manager.snapshot_count(),
                total_size_bytes: snapshot_manager.total_snapshot_size(),
                latest_snapshot_id: snapshot_manager.get_latest_snapshot().map(|info| info.id),
            }
        } else {
            SnapshotStats::default()
        }
    }

    pub fn mark_flushed(&self, lsn: Lsn) {
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = Instant::now();
    }

    pub fn mark_checkpointed(&self, lsn: Lsn) {
        if let Some(ref wal) = self.wal_manager {
            let _ = wal.read().set_current_lsn(lsn);
        }
        *self.last_checkpoint_lsn.write() = lsn;
        *self.last_checkpoint_time.write() = Instant::now();
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = Instant::now();
    }
}

#[derive(Debug, Clone)]
pub struct CheckpointData {
    pub vertex_count: u64,
    pub edge_count: u64,
    pub data_size: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SnapshotStats {
    pub snapshot_count: usize,
    pub total_size_bytes: u64,
    pub latest_snapshot_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_persistence_config_default() {
        let config = PersistenceConfig::default();
        assert_eq!(config.data_dir, PathBuf::from("data"));
        assert_eq!(config.auto_flush_interval, Duration::from_secs(60));
    }

    #[test]
    fn test_should_flush_and_checkpoint_track_lsn_progress() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            data_dir: temp_dir.path().join("data"),
            wal_dir: temp_dir.path().join("wal"),
            checkpoint_dir: temp_dir.path().join("checkpoint"),
            snapshot_dir: temp_dir.path().join("snapshots"),
            auto_flush_interval: Duration::from_secs(3600),
            auto_checkpoint_interval: Duration::from_secs(3600),
            checkpoint_threshold: 8,
            max_wal_size: 16,
            enable_snapshots: false,
            snapshot_interval: Duration::from_secs(3600),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
        };

        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");
        assert!(!coordinator.should_flush());
        assert!(!coordinator.should_checkpoint());

        {
            let wal = coordinator.wal_manager().expect("WAL should be enabled");
            wal.write()
                .truncate(Lsn::new(12))
                .expect("Failed to update LSN");
        }

        assert!(coordinator.should_flush());
        assert!(coordinator.should_checkpoint());

        coordinator.mark_checkpointed(Lsn::new(12));
        assert!(!coordinator.should_flush());
        assert!(!coordinator.should_checkpoint());
    }

    #[test]
    fn test_mark_checkpointed_updates_wal_lsn() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            data_dir: temp_dir.path().join("data"),
            wal_dir: temp_dir.path().join("wal"),
            checkpoint_dir: temp_dir.path().join("checkpoint"),
            snapshot_dir: temp_dir.path().join("snapshots"),
            auto_flush_interval: Duration::from_secs(3600),
            auto_checkpoint_interval: Duration::from_secs(3600),
            checkpoint_threshold: 8,
            max_wal_size: 16,
            enable_snapshots: false,
            snapshot_interval: Duration::from_secs(3600),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
        };

        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");

        {
            let wal = coordinator.wal_manager().expect("WAL should be enabled");
            wal.write()
                .truncate(Lsn::new(12))
                .expect("Failed to update LSN");
        }

        coordinator.mark_checkpointed(Lsn::new(24));

        assert_eq!(
            coordinator
                .wal_manager()
                .expect("WAL should be enabled")
                .read()
                .current_lsn(),
            Lsn::new(24)
        );
    }
}

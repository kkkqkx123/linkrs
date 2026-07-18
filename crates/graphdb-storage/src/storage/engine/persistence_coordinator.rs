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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::RwLock;

use graphdb_sync::sync::checkpoint_manifest::{
    CheckpointManifest, CheckpointManifestManager, IndexManifestRef, StorageSnapshotRef,
};

use crate::core::types::Timestamp;
use crate::core::{StorageError, StorageResult};
use crate::storage::engine::config::PropertyGraphConfig;
use crate::storage::engine::snapshot_manager::{SnapshotManager, SnapshotOptions};
use crate::storage::engine::WalManager;
use crate::transaction::wal::{CheckpointManager, Lsn, SyncPolicy, WalConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
    Idle,
    Checkpointing,
    Snapshotting,
}

/// Controlled failure points used by recovery tests and operational drills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersistenceFaultPoint {
    /// Fail before the checkpoint's durable data is written.
    CheckpointRedoBefore,
    /// Fail after data flush and before checkpoint metadata is written.
    CheckpointIntentMid,
    /// Fail after metadata write and before the checkpoint directory is committed.
    CheckpointCommitMid,
    /// Fail after the checkpoint directory has been fsynced.
    CheckpointFsyncAfter,
    /// Fail immediately before the combined checkpoint manifest becomes visible.
    CheckpointVisibilityPublish,
    RecoveryScan,
}

struct PersistenceStateGuard<'a> {
    state: &'a RwLock<PersistenceState>,
}

impl Drop for PersistenceStateGuard<'_> {
    fn drop(&mut self) {
        *self.state.write() = PersistenceState::Idle;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckpointFileEntry {
    path: PathBuf,
    size: u64,
    checksum: u32,
}

const CHECKPOINT_FORMAT_VERSION: u32 = 2;

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
    /// Property graph resource and maintenance configuration.
    pub property_graph_config: PropertyGraphConfig,
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
            property_graph_config: PropertyGraphConfig::default(),
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
            property_graph_config: PropertyGraphConfig::default(),
            ..Default::default()
        }
    }

    pub fn with_property_graph_config(mut self, config: PropertyGraphConfig) -> Self {
        self.property_graph_config = config;
        self
    }
}

pub struct PersistenceCoordinator {
    config: PersistenceConfig,
    wal_manager: Option<Arc<RwLock<WalManager>>>,
    checkpoint_manager: RwLock<CheckpointManager>,
    snapshot_manager: Option<Arc<SnapshotManager>>,
    manifest_manager: CheckpointManifestManager,
    last_checkpoint_time: RwLock<Instant>,
    last_flush_time: RwLock<Instant>,
    last_checkpoint_lsn: RwLock<Lsn>,
    last_flush_lsn: RwLock<Lsn>,
    last_snapshot_time: RwLock<Option<SystemTime>>,
    last_checkpoint_error: RwLock<Option<String>>,
    last_snapshot_error: RwLock<Option<String>>,
    state: RwLock<PersistenceState>,
    fault_points: Arc<RwLock<HashSet<PersistenceFaultPoint>>>,
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

        Self::cleanup_temporary_checkpoints(&config.checkpoint_dir)?;
        let manifest_dir = config.checkpoint_dir.join("manifests");
        let manifest_manager = CheckpointManifestManager::new(&manifest_dir);
        manifest_manager.init().map_err(|error| {
            StorageError::db_error(format!("Failed to init manifest manager: {error}"))
        })?;
        let published_sequence = Self::latest_published_sequence(&manifest_manager)?;
        Self::cleanup_unpublished_checkpoints(&config.checkpoint_dir, published_sequence)?;

        let mut checkpoint_manager =
            CheckpointManager::new(&config.wal_dir, &config.checkpoint_dir, None);
        checkpoint_manager.init().map_err(|e| {
            crate::core::StorageError::db_error(format!("Failed to init checkpoint manager: {}", e))
        })?;
        checkpoint_manager
            .adopt_published_sequence(published_sequence)
            .map_err(|error| {
                StorageError::db_error(format!(
                    "Failed to reconcile published checkpoints: {}",
                    error
                ))
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
            manifest_manager,
            last_checkpoint_time: RwLock::new(Instant::now()),
            last_flush_time: RwLock::new(Instant::now()),
            last_checkpoint_lsn: RwLock::new(Lsn::ZERO),
            last_flush_lsn: RwLock::new(Lsn::ZERO),
            last_snapshot_time: RwLock::new(None),
            last_checkpoint_error: RwLock::new(None),
            last_snapshot_error: RwLock::new(None),
            state: RwLock::new(PersistenceState::Idle),
            fault_points: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Enable a deterministic failure at a persistence boundary.
    pub fn inject_failure(&self, point: PersistenceFaultPoint) {
        self.fault_points.write().insert(point);
    }

    pub fn clear_injected_failures(&self) {
        self.fault_points.write().clear();
    }

    fn fail_if_injected(&self, point: PersistenceFaultPoint) -> StorageResult<()> {
        if self.fault_points.read().contains(&point) {
            return Err(StorageError::io_error(format!(
                "injected persistence failure at {:?}",
                point
            )));
        }
        Ok(())
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

    fn enter_state(&self, state: PersistenceState) -> StorageResult<PersistenceStateGuard<'_>> {
        let mut current = self.state.write();
        if *current != PersistenceState::Idle {
            return Err(StorageError::invalid_operation(format!(
                "persistence operation is already active: {:?}",
                *current
            )));
        }
        *current = state;
        drop(current);
        Ok(PersistenceStateGuard { state: &self.state })
    }

    fn cleanup_temporary_checkpoints(checkpoint_dir: &Path) -> StorageResult<()> {
        for entry in std::fs::read_dir(checkpoint_dir)? {
            let entry = entry?;
            let path = entry.path();
            let is_temporary = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("checkpoint_") && name.ends_with(".tmp"));
            if is_temporary {
                if path.is_dir() {
                    std::fs::remove_dir_all(path)?;
                } else {
                    std::fs::remove_file(path)?;
                }
            }
        }
        Ok(())
    }

    fn latest_published_sequence(
        manifest_manager: &CheckpointManifestManager,
    ) -> StorageResult<u64> {
        manifest_manager
            .load_latest()
            .map_err(StorageError::db_error)
            .map(|manifest| manifest.map_or(0, |manifest| manifest.checkpoint_id))
    }

    /// A checkpoint directory is recoverable only after its combined manifest
    /// is visible. Remove directories left behind by a crash after rename but
    /// before that final publication fence.
    fn cleanup_unpublished_checkpoints(
        checkpoint_dir: &Path,
        latest_published_sequence: u64,
    ) -> StorageResult<()> {
        for entry in std::fs::read_dir(checkpoint_dir)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let Some(sequence) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("checkpoint_"))
                .and_then(|value| value.parse::<u64>().ok())
            else {
                continue;
            };
            if sequence > latest_published_sequence {
                std::fs::remove_dir_all(path)?;
            }
        }
        Ok(())
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
        let result = self.create_checkpoint_inner(flush_data, timestamp);
        match &result {
            Ok(_) => *self.last_checkpoint_error.write() = None,
            Err(error) => *self.last_checkpoint_error.write() = Some(error.to_string()),
        }
        result
    }

    fn create_checkpoint_inner(
        &self,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
    ) -> StorageResult<CheckpointStats> {
        let start = Instant::now();
        let _state_guard = self.enter_state(PersistenceState::Checkpointing)?;

        let wal_lsn = {
            match &self.wal_manager {
                Some(wal) => {
                    // A checkpoint is a durability fence. Flush any accepted
                    // WAL bytes before exporting the in-memory state so the
                    // checkpoint data and its WAL boundary describe the same
                    // durable prefix.
                    wal.read().sync()?;
                    wal.read().durable_lsn()
                }
                None => Lsn::ZERO,
            }
        };

        log::info!(
            "Creating checkpoint at timestamp {}, LSN {}",
            timestamp,
            wal_lsn
        );

        let checkpoint = {
            let published_sequence = Self::latest_published_sequence(&self.manifest_manager)?;
            let mut cm = self.checkpoint_manager.write();
            cm.adopt_published_sequence(published_sequence)
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to reconcile published checkpoints: {}",
                        error
                    ))
                })?;
            cm.prepare_checkpoint(timestamp, wal_lsn).map_err(|e| {
                crate::core::StorageError::db_error(format!("Failed to create checkpoint: {}", e))
            })?
        };

        let checkpoint_dir = self
            .config
            .checkpoint_dir
            .join(format!("checkpoint_{}", checkpoint.seq));
        let temporary_dir = self
            .config
            .checkpoint_dir
            .join(format!("checkpoint_{}.tmp", checkpoint.seq));
        if temporary_dir.exists() {
            std::fs::remove_dir_all(&temporary_dir)?;
        }
        if checkpoint_dir.exists() {
            let is_published = self
                .manifest_manager
                .load(checkpoint.seq)
                .map_err(|error| {
                    StorageError::db_error(format!(
                        "Failed to inspect existing checkpoint {}: {}",
                        checkpoint.seq, error
                    ))
                })?
                .is_some();
            if is_published {
                return Err(StorageError::invalid_operation(format!(
                    "checkpoint {} already exists",
                    checkpoint.seq
                )));
            }

            // A directory without a published manifest is not a recoverable
            // checkpoint. It may be a stale directory left by an interrupted
            // recovery attempt, so remove it before reusing the sequence.
            std::fs::remove_dir_all(&checkpoint_dir)?;
            Self::sync_directory(&self.config.checkpoint_dir)?;
        }
        std::fs::create_dir(&temporary_dir)?;

        self.fail_if_injected(PersistenceFaultPoint::CheckpointRedoBefore)?;
        let data = flush_data(&temporary_dir, timestamp)?;
        self.fail_if_injected(PersistenceFaultPoint::CheckpointIntentMid)?;
        let files = Self::collect_checkpoint_files(&temporary_dir)?;
        self.save_checkpoint_metadata(&temporary_dir, &checkpoint, &data, &files)?;
        self.fail_if_injected(PersistenceFaultPoint::CheckpointCommitMid)?;
        Self::sync_tree(&temporary_dir)?;
        std::fs::rename(&temporary_dir, &checkpoint_dir)?;
        Self::sync_directory(&self.config.checkpoint_dir)?;
        self.fail_if_injected(PersistenceFaultPoint::CheckpointFsyncAfter)?;

        {
            let mut cm = self.checkpoint_manager.write();
            cm.publish_checkpoint(&checkpoint).map_err(|e| {
                StorageError::db_error(format!("Failed to publish checkpoint: {}", e))
            })?;
        }

        // Publication order is part of the recovery protocol:
        // checkpoint files -> directory fsync -> manager metadata -> WAL
        // boundary -> snapshot -> outbox marker (the wrapper performs the
        // last step). Retention only deletes points after all references are
        // collected, so a failed later step never exposes partial data.
        if let Some(ref wal) = self.wal_manager {
            wal.read().set_checkpoint_seq(checkpoint.seq)?;
        }

        self.mark_checkpointed(wal_lsn);

        let snapshot_created = if self.should_snapshot() {
            *self.state.write() = PersistenceState::Snapshotting;
            if let Some(ref snapshot_manager) = self.snapshot_manager {
                let snapshot_options = SnapshotOptions::default();
                match snapshot_manager.create_snapshot(
                    crate::storage::engine::snapshot_manager::CreateSnapshotParams {
                        source_dir: checkpoint_dir.clone(),
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
                        *self.last_snapshot_error.write() = None;
                        true
                    }
                    Err(e) => {
                        log::error!("Failed to create snapshot: {}", e);
                        *self.last_snapshot_error.write() = Some(e.to_string());
                        false
                    }
                }
            } else {
                false
            }
        } else {
            false
        };

        self.fail_if_injected(PersistenceFaultPoint::CheckpointVisibilityPublish)?;
        self.publish_checkpoint_manifest(&checkpoint, &data, &checkpoint_dir, wal_lsn)?;

        if let Some(ref wal) = self.wal_manager {
            let safe_lsn = self.manifest_manager.latest_safe_lsn().map_err(|error| {
                StorageError::db_error(format!("Failed to get safe LSN: {}", error))
            })?;
            let safe_wal_lsn = Lsn::new(safe_lsn.get());
            wal.read().truncate(safe_wal_lsn)?;
        }

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
        files: &[CheckpointFileEntry],
    ) -> StorageResult<()> {
        use std::fs::File;
        use std::io::Write;

        let metadata_path = dir.join("checkpoint.meta");
        let mut file = File::create(metadata_path)?;

        writeln!(file, "format_version={}", CHECKPOINT_FORMAT_VERSION)?;
        writeln!(file, "checkpoint_id={}", checkpoint.seq)?;
        writeln!(file, "timestamp={}", checkpoint.timestamp)?;
        writeln!(file, "wal_lsn={}", checkpoint.lsn.0)?;
        writeln!(file, "vertex_count={}", data.vertex_count)?;
        writeln!(file, "edge_count={}", data.edge_count)?;
        writeln!(file, "data_size={}", data.data_size)?;
        writeln!(file, "created_at={:?}", SystemTime::now())?;
        for entry in files {
            writeln!(
                file,
                "file={}|{}|{}",
                entry.path.display(),
                entry.size,
                entry.checksum
            )?;
        }
        file.sync_all()?;

        Ok(())
    }

    fn collect_checkpoint_files(root: &Path) -> StorageResult<Vec<CheckpointFileEntry>> {
        fn visit(
            root: &Path,
            directory: &Path,
            entries: &mut Vec<CheckpointFileEntry>,
        ) -> StorageResult<()> {
            for item in std::fs::read_dir(directory)? {
                let item = item?;
                let path = item.path();
                if path.is_dir() {
                    visit(root, &path, entries)?;
                } else if path.is_file() {
                    let relative = path.strip_prefix(root).map_err(|error| {
                        StorageError::invalid_operation(format!(
                            "checkpoint file is outside its root: {}",
                            error
                        ))
                    })?;
                    let bytes = std::fs::read(&path)?;
                    entries.push(CheckpointFileEntry {
                        path: relative.to_path_buf(),
                        size: bytes.len() as u64,
                        checksum: crc32fast::hash(&bytes),
                    });
                }
            }
            Ok(())
        }

        let mut entries = Vec::new();
        visit(root, root, &mut entries)?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn sync_tree(root: &Path) -> StorageResult<()> {
        fn visit(directory: &Path) -> StorageResult<()> {
            for item in std::fs::read_dir(directory)? {
                let path = item?.path();
                if path.is_dir() {
                    visit(&path)?;
                } else if path.is_file() {
                    std::fs::File::open(path)?.sync_all()?;
                }
            }
            PersistenceCoordinator::sync_directory(directory)
        }

        visit(root)
    }

    fn sync_directory(directory: &Path) -> StorageResult<()> {
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    }

    fn verify_checkpoint_files(
        checkpoint_dir: &Path,
        files: &[CheckpointFileEntry],
    ) -> StorageResult<()> {
        for entry in files {
            if entry.path.is_absolute()
                || entry
                    .path
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
            {
                return Err(StorageError::deserialize_error(format!(
                    "Invalid checkpoint file path: {}",
                    entry.path.display()
                )));
            }
            let path = checkpoint_dir.join(&entry.path);
            let bytes = std::fs::read(&path)?;
            if bytes.len() as u64 != entry.size || crc32fast::hash(&bytes) != entry.checksum {
                return Err(StorageError::deserialize_error(format!(
                    "Checkpoint file verification failed: {}",
                    entry.path.display()
                )));
            }
        }
        Ok(())
    }

    pub fn load_latest_checkpoint(
        &self,
        load_data: impl FnOnce(&Path) -> StorageResult<()>,
    ) -> StorageResult<Option<CheckpointInfo>> {
        self.fail_if_injected(PersistenceFaultPoint::RecoveryScan)?;
        let checkpoints_dir = &self.config.checkpoint_dir;

        if !checkpoints_dir.exists() {
            return Ok(None);
        }

        let Some(published_manifest) = self
            .manifest_manager
            .load_latest()
            .map_err(StorageError::db_error)?
        else {
            return Ok(None);
        };
        let published_checkpoint = published_manifest.storage_snapshot.checkpoint_seq;
        let mut checkpoints: Vec<(u64, PathBuf)> = std::fs::read_dir(checkpoints_dir)?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_dir() && path.join("checkpoint.meta").is_file() {
                    let name = path.file_name()?.to_string_lossy();
                    if name.starts_with("checkpoint_") {
                        let id: u64 = name.trim_start_matches("checkpoint_").parse().ok()?;
                        (id == published_checkpoint).then_some((id, path))
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
            let (info, files) = self.load_checkpoint_metadata(checkpoint_path)?;
            Self::verify_checkpoint_files(checkpoint_path, &files)?;

            load_data(checkpoint_path)?;

            if let Some(ref wal) = self.wal_manager {
                wal.read().set_checkpoint_seq(info.checkpoint_id)?;
                wal.read().set_recovery_baseline_lsn(info.lsn)?;
            }

            return Ok(Some(info));
        }

        Ok(None)
    }

    fn load_checkpoint_metadata(
        &self,
        dir: &Path,
    ) -> StorageResult<(CheckpointInfo, Vec<CheckpointFileEntry>)> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let metadata_path = dir.join("checkpoint.meta");
        let file = File::open(metadata_path)?;
        let reader = BufReader::new(file);

        let mut checkpoint_id: Option<u64> = None;
        let mut lsn: Option<u64> = None;
        let mut timestamp: Option<u32> = None;
        let mut format_version: Option<u32> = None;
        let mut files = Vec::new();

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
                "format_version" => {
                    format_version = Some(parts[1].parse().map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "Invalid format_version in checkpoint metadata: {}",
                            e
                        ))
                    })?);
                }
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
                "file" => {
                    let mut fields = parts[1].rsplitn(3, '|');
                    let checksum = fields.next().and_then(|value| value.parse::<u32>().ok());
                    let size = fields.next().and_then(|value| value.parse::<u64>().ok());
                    let path = fields.next().map(PathBuf::from);
                    match (path, size, checksum) {
                        (Some(path), Some(size), Some(checksum)) => {
                            files.push(CheckpointFileEntry {
                                path,
                                size,
                                checksum,
                            })
                        }
                        _ => {
                            return Err(StorageError::deserialize_error(
                                "Invalid file entry in checkpoint metadata".to_string(),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }

        if format_version != Some(CHECKPOINT_FORMAT_VERSION) {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported checkpoint format version: {:?}",
                format_version
            )));
        }
        if files.is_empty() {
            return Err(StorageError::deserialize_error(
                "Checkpoint metadata contains no files".to_string(),
            ));
        }

        let checkpoint_id = checkpoint_id.ok_or_else(|| {
            StorageError::deserialize_error(
                "Missing checkpoint_id in checkpoint metadata".to_string(),
            )
        })?;
        let lsn = lsn.ok_or_else(|| {
            StorageError::deserialize_error("Missing wal_lsn in checkpoint metadata".to_string())
        })?;

        Ok((
            CheckpointInfo {
                checkpoint_id,
                lsn: Lsn::new(lsn),
                timestamp: timestamp.unwrap_or(0),
            },
            files,
        ))
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

    /// Remove published checkpoints older than the retention limit while
    /// keeping the newest valid recovery points.
    pub fn cleanup_old_checkpoints(&self, max_checkpoints: usize) -> StorageResult<usize> {
        let keep = max_checkpoints.max(1);
        let current_sequence = self.checkpoint_manager.read().current_seq();
        let retained_by_snapshot = self
            .snapshot_manager
            .as_ref()
            .map(|manager| manager.retained_checkpoint_sequences())
            .unwrap_or_default();
        let mut checkpoints: Vec<(u64, PathBuf)> = std::fs::read_dir(&self.config.checkpoint_dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                let sequence = name.strip_prefix("checkpoint_")?.parse::<u64>().ok()?;
                path.is_dir().then_some((sequence, path))
            })
            .collect();
        checkpoints.sort_by_key(|(sequence, _)| *sequence);
        let remove_count = checkpoints.len().saturating_sub(keep);
        let mut removed = 0;
        for (_sequence, path) in checkpoints.into_iter().filter(|(sequence, _)| {
            *sequence != current_sequence && !retained_by_snapshot.contains(sequence)
        }) {
            if removed >= remove_count {
                break;
            }
            std::fs::remove_dir_all(path)?;
            removed += 1;
        }
        if removed > 0 {
            Self::sync_directory(&self.config.checkpoint_dir)?;
        }
        Ok(removed)
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

    pub fn diagnostics(&self) -> PersistenceDiagnostics {
        let temporary_checkpoint_count = std::fs::read_dir(&self.config.checkpoint_dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.ends_with(".tmp"))
                    })
                    .count()
            })
            .unwrap_or(0);
        let manifest_safe_lsn = self
            .manifest_manager
            .latest_safe_lsn()
            .map(|lsn| Lsn::new(lsn.get()))
            .unwrap_or_else(|_| *self.last_checkpoint_lsn.read());
        PersistenceDiagnostics {
            state: *self.state.read(),
            checkpoint_sequence: self.checkpoint_manager.read().current_seq(),
            safe_lsn: manifest_safe_lsn,
            last_checkpoint_error: self.last_checkpoint_error.read().clone(),
            last_snapshot_error: self.last_snapshot_error.read().clone(),
            temporary_checkpoint_count,
            catalog_lock_acquisitions: 0,
            catalog_lock_wait_nanos: 0,
            catalog_lock_hold_nanos: 0,
            catalog_lock_contentions: 0,
            catalog_lock_by_operation: Vec::new(),
        }
    }

    pub fn mark_flushed(&self, lsn: Lsn) {
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = Instant::now();
    }

    pub fn mark_checkpointed(&self, lsn: Lsn) {
        *self.last_checkpoint_lsn.write() = lsn;
        *self.last_checkpoint_time.write() = Instant::now();
        *self.last_flush_lsn.write() = lsn;
        *self.last_flush_time.write() = Instant::now();
    }

    /// Publish a combined checkpoint manifest that atomically references the
    /// storage snapshot, outbox snapshot (if provided), and index manifests.
    ///
    /// This implements the Phase 3 requirement for atomic checkpoint manifest
    /// publication. The manifest is written to a temporary file, synced, then
    /// atomically renamed. Only after successful publication is WAL truncated
    /// to the common safe LSN.
    fn publish_checkpoint_manifest(
        &self,
        checkpoint: &crate::transaction::wal::Checkpoint,
        data: &CheckpointData,
        checkpoint_dir: &Path,
        wal_lsn: Lsn,
    ) -> StorageResult<()> {
        let storage_snapshot_ref = StorageSnapshotRef {
            path: checkpoint_dir.to_path_buf(),
            size_bytes: data.data_size,
            checksum: crc32fast::hash(
                &std::fs::read(checkpoint_dir.join("checkpoint.meta")).unwrap_or_default(),
            ),
            checkpoint_seq: checkpoint.seq,
            vertex_count: data.vertex_count,
            edge_count: data.edge_count,
        };

        let storage_lsn = graphdb_core::core::types::CommitLsn::new(wal_lsn.into());
        let work_dir = self
            .config
            .data_dir
            .parent()
            .unwrap_or(&self.config.data_dir);
        let outbox_snapshot =
            graphdb_sync::sync::find_latest_snapshot(&work_dir.join("outbox_snapshots"))
                .filter(|snapshot| snapshot.materialized_lsn >= storage_lsn)
                .map(|snapshot| CheckpointManifest::outbox_snapshot_from(&snapshot));
        let index_manifests = Self::collect_index_manifest_refs(checkpoint_dir)?;

        let manifest = CheckpointManifest::new(
            checkpoint.seq,
            storage_lsn,
            storage_snapshot_ref,
            outbox_snapshot,
            index_manifests,
        );

        self.manifest_manager.publish(&manifest).map_err(|error| {
            StorageError::db_error(format!("Failed to publish manifest: {}", error))
        })?;

        log::info!(
            "Published checkpoint manifest {} with safe LSN {}",
            checkpoint.seq,
            manifest.safe_lsn
        );

        Ok(())
    }

    fn collect_index_manifest_refs(root: &Path) -> StorageResult<Vec<IndexManifestRef>> {
        fn visit(directory: &Path, refs: &mut Vec<IndexManifestRef>) -> StorageResult<()> {
            for entry in std::fs::read_dir(directory)? {
                let path = entry?.path();
                if path.is_dir() {
                    visit(&path, refs)?;
                    continue;
                }
                if path
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    != Some("native_index_manifests")
                    || path.extension().and_then(|extension| extension.to_str()) != Some("json")
                {
                    continue;
                }
                let manifest = crate::storage::index::manifest::IndexManifest::load(&path)?;
                let bytes = std::fs::read(&path)?;
                refs.push(IndexManifestRef {
                    space_id: manifest.space_id,
                    index_id: manifest.index_id,
                    generation: manifest.generation,
                    path,
                    size_bytes: bytes.len() as u64,
                    checksum: crc32fast::hash(&bytes),
                });
            }
            Ok(())
        }

        let mut refs = Vec::new();
        visit(root, &mut refs)?;
        refs.sort_by_key(|reference| reference.index_id);
        Ok(refs)
    }

    /// Get the latest safe LSN from the published manifest.
    pub fn latest_safe_lsn(&self) -> StorageResult<graphdb_core::core::types::CommitLsn> {
        self.manifest_manager
            .latest_safe_lsn()
            .map_err(StorageError::db_error)
    }

    /// Load the latest published checkpoint manifest.
    pub fn load_latest_manifest(&self) -> StorageResult<Option<CheckpointManifest>> {
        self.manifest_manager
            .load_latest()
            .map_err(StorageError::db_error)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistenceDiagnostics {
    pub state: PersistenceState,
    pub checkpoint_sequence: u64,
    pub safe_lsn: Lsn,
    pub last_checkpoint_error: Option<String>,
    pub last_snapshot_error: Option<String>,
    pub temporary_checkpoint_count: usize,
    /// Number of catalog lock acquisitions observed by the storage engine.
    pub catalog_lock_acquisitions: u64,
    /// Total time spent waiting for catalog locks, in nanoseconds.
    pub catalog_lock_wait_nanos: u64,
    /// Total time catalog guards were held, in nanoseconds.
    pub catalog_lock_hold_nanos: u64,
    /// Number of catalog acquisitions that observed measurable contention.
    pub catalog_lock_contentions: u64,
    /// Lock metrics split by catalog operation type.
    pub catalog_lock_by_operation: Vec<CatalogLockDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogLockDiagnostic {
    pub operation: String,
    pub acquisitions: u64,
    pub wait_nanos: u64,
    pub hold_nanos: u64,
    pub contentions: u64,
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
            property_graph_config: PropertyGraphConfig::default(),
        };

        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");
        assert!(!coordinator.should_flush());
        assert!(!coordinator.should_checkpoint());

        {
            let wal = coordinator.wal_manager().expect("WAL should be enabled");
            wal.write()
                .set_current_lsn(Lsn::new(12))
                .expect("test LSN should be accepted");
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
            property_graph_config: PropertyGraphConfig::default(),
        };

        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");

        {
            let wal = coordinator.wal_manager().expect("WAL should be enabled");
            wal.write()
                .set_current_lsn(Lsn::new(12))
                .expect("test LSN should be accepted");
        }

        coordinator.mark_checkpointed(Lsn::new(24));

        assert_eq!(
            coordinator
                .wal_manager()
                .expect("WAL should be enabled")
                .read()
                .current_lsn(),
            Lsn::new(12)
        );
        assert_eq!(*coordinator.last_checkpoint_lsn.read(), Lsn::new(24));
    }

    #[test]
    fn snapshot_uses_the_completed_checkpoint_as_its_only_source() {
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
            enable_snapshots: true,
            snapshot_interval: Duration::ZERO,
            enable_wal: false,
            sync_policy: None,
            property_graph_config: PropertyGraphConfig::default(),
        };
        std::fs::create_dir_all(&config.data_dir).expect("Failed to create main data dir");
        std::fs::write(config.data_dir.join("stale.txt"), b"stale")
            .expect("Failed to write stale main data");

        let snapshots_dir = config.snapshot_dir.clone();
        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");
        let stats = coordinator
            .create_checkpoint(
                |checkpoint_dir, _| {
                    let checkpoint_data = checkpoint_dir.join("data");
                    std::fs::create_dir_all(&checkpoint_data)?;
                    std::fs::write(checkpoint_data.join("fresh.txt"), b"fresh")?;
                    Ok(CheckpointData {
                        vertex_count: 1,
                        edge_count: 1,
                        data_size: 5,
                    })
                },
                7,
            )
            .expect("Failed to create checkpoint");

        assert!(stats.snapshot_created);
        let snapshot_dir = snapshots_dir.join(format!("snapshot_{:010}", stats.checkpoint_id));
        assert_eq!(
            std::fs::read(snapshot_dir.join("data/fresh.txt"))
                .expect("Snapshot should contain checkpoint data"),
            b"fresh"
        );
        assert!(!snapshot_dir.join("stale.txt").exists());
        assert!(snapshot_dir.join("checkpoint.meta").exists());

        let info = coordinator
            .snapshot_manager
            .as_ref()
            .and_then(|manager| manager.get_snapshot(stats.checkpoint_id))
            .expect("Snapshot metadata should exist");
        assert_eq!(info.checkpoint_seq, stats.checkpoint_id);
        assert_eq!(info.wal_lsn, 0);
        assert_eq!(info.vertex_count, 1);
        assert_eq!(info.edge_count, 1);
    }

    #[test]
    fn failed_checkpoint_is_not_published_and_state_returns_to_idle() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: false,
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        let checkpoint_dir = config.checkpoint_dir.clone();
        let coordinator =
            PersistenceCoordinator::new(config.clone()).expect("Failed to create coordinator");

        let result = coordinator.create_checkpoint(
            |temporary_dir, _| {
                std::fs::write(temporary_dir.join("partial.data"), b"partial")?;
                Err(StorageError::db_error("injected flush failure"))
            },
            11,
        );

        assert!(result.is_err());
        assert_eq!(*coordinator.state.read(), PersistenceState::Idle);
        assert_eq!(coordinator.checkpoint_manager.read().current_seq(), 0);
        assert!(!checkpoint_dir.join("checkpoint_1").exists());
        assert!(checkpoint_dir.join("checkpoint_1.tmp").exists());

        drop(coordinator);
        let reopened = PersistenceCoordinator::new(config).expect("Failed to reopen coordinator");
        assert!(!checkpoint_dir.join("checkpoint_1.tmp").exists());
        assert_eq!(reopened.checkpoint_manager.read().current_seq(), 0);
    }

    #[test]
    fn published_checkpoint_manifest_detects_file_corruption() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: false,
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        let checkpoint_dir = config.checkpoint_dir.clone();
        let coordinator =
            PersistenceCoordinator::new(config).expect("Failed to create coordinator");
        coordinator
            .create_checkpoint(
                |temporary_dir, _| {
                    let data_dir = temporary_dir.join("data");
                    std::fs::create_dir(&data_dir)?;
                    std::fs::write(data_dir.join("table.data"), b"valid")?;
                    Ok(CheckpointData {
                        vertex_count: 1,
                        edge_count: 0,
                        data_size: 5,
                    })
                },
                12,
            )
            .expect("Checkpoint should be published");

        std::fs::write(
            checkpoint_dir.join("checkpoint_1/data/table.data"),
            b"corrupt",
        )
        .expect("Failed to corrupt checkpoint file");
        let result = coordinator.load_latest_checkpoint(|_| Ok(()));
        assert!(result.is_err());
    }

    #[test]
    fn startup_removes_checkpoint_directories_without_a_published_manifest() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: false,
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        std::fs::create_dir_all(config.checkpoint_dir.join("checkpoint_7.tmp"))
            .expect("Failed to create temporary checkpoint");
        std::fs::create_dir_all(config.checkpoint_dir.join("checkpoint_6"))
            .expect("Failed to create unpublished checkpoint placeholder");

        let _coordinator =
            PersistenceCoordinator::new(config.clone()).expect("Failed to create coordinator");

        assert!(!config.checkpoint_dir.join("checkpoint_7.tmp").exists());
        assert!(!config.checkpoint_dir.join("checkpoint_6").exists());
    }

    #[test]
    fn checkpoint_retention_keeps_the_latest_published_point() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: false,
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        let coordinator = PersistenceCoordinator::new(config.clone()).expect("coordinator");
        for sequence in 1..=3 {
            coordinator
                .create_checkpoint(
                    |temporary_dir, _| {
                        std::fs::write(
                            temporary_dir.join("table.data"),
                            sequence.to_string().as_bytes(),
                        )?;
                        Ok(CheckpointData {
                            vertex_count: sequence as u64,
                            edge_count: 0,
                            data_size: 1,
                        })
                    },
                    sequence,
                )
                .expect("checkpoint should publish");
        }
        assert_eq!(coordinator.cleanup_old_checkpoints(1).expect("cleanup"), 2);
        assert!(!config.checkpoint_dir.join("checkpoint_1").exists());
        assert!(!config.checkpoint_dir.join("checkpoint_2").exists());
        assert!(config.checkpoint_dir.join("checkpoint_3").exists());
    }

    #[test]
    fn checkpoint_retention_keeps_latest() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: false,
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        let coordinator = PersistenceCoordinator::new(config.clone()).expect("coordinator");
        for sequence in 1..=3 {
            coordinator
                .create_checkpoint(
                    |temporary_dir, _| {
                        std::fs::write(temporary_dir.join("table.data"), b"data")?;
                        Ok(CheckpointData {
                            vertex_count: sequence as u64,
                            edge_count: 0,
                            data_size: 4,
                        })
                    },
                    sequence,
                )
                .expect("checkpoint should publish");
        }

        assert_eq!(coordinator.cleanup_old_checkpoints(1).expect("cleanup"), 2);
        assert!(config.checkpoint_dir.join("checkpoint_3").exists());
    }

    #[test]
    fn every_checkpoint_crash_boundary_recovers_without_publishing_partial_state() {
        let fault_points = [
            PersistenceFaultPoint::CheckpointRedoBefore,
            PersistenceFaultPoint::CheckpointIntentMid,
            PersistenceFaultPoint::CheckpointCommitMid,
            PersistenceFaultPoint::CheckpointFsyncAfter,
            PersistenceFaultPoint::CheckpointVisibilityPublish,
        ];
        for point in fault_points {
            let temp_dir = TempDir::new().expect("Failed to create temp dir");
            let config = PersistenceConfig {
                enable_snapshots: false,
                enable_wal: false,
                ..PersistenceConfig::for_work_dir(temp_dir.path())
            };
            let coordinator = PersistenceCoordinator::new(config.clone()).expect("coordinator");
            coordinator.inject_failure(point);
            let result = coordinator.create_checkpoint(
                |temporary_dir, _| {
                    std::fs::write(temporary_dir.join("table.data"), b"data")?;
                    Ok(CheckpointData {
                        vertex_count: 1,
                        edge_count: 0,
                        data_size: 4,
                    })
                },
                1,
            );
            assert!(result.is_err(), "fault point {point:?} should fail");
            assert_eq!(*coordinator.state.read(), PersistenceState::Idle);
            assert!(coordinator.diagnostics().last_checkpoint_error.is_some());
            assert!(
                coordinator
                    .manifest_manager
                    .load_latest()
                    .expect("manifest lookup should succeed")
                    .is_none(),
                "fault point {point:?} must not publish a checkpoint"
            );
            drop(coordinator);

            let recovered = PersistenceCoordinator::new(config.clone())
                .expect("recovery should discard unpublished checkpoint state");
            assert!(
                !config.checkpoint_dir.join("checkpoint_1").exists(),
                "recovery should remove the unpublished checkpoint for {point:?}"
            );
            recovered
                .create_checkpoint(
                    |temporary_dir, _| {
                        std::fs::write(temporary_dir.join("table.data"), b"data")?;
                        Ok(CheckpointData {
                            vertex_count: 1,
                            edge_count: 0,
                            data_size: 4,
                        })
                    },
                    2,
                )
                .expect("a checkpoint should succeed after recovery");
            assert!(
                recovered
                    .manifest_manager
                    .load_latest()
                    .expect("manifest lookup should succeed")
                    .is_some(),
                "recovered checkpoint should be visible for {point:?}"
            );
        }
    }
}

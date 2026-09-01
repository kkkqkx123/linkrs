use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use graphdb_core::stats::CheckpointTriggerReason;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult};
use graphdb_sync::checkpoint_manifest::{
    CheckpointManifest, CheckpointManifestManager, IndexManifestRef,
};
use graphdb_transaction::wal::Lsn;

use crate::engine::snapshot_manager::SnapshotOptions;
use crate::persistence::dirty_page::{CheckpointStrategy, IncrementalCheckpointMeta};

pub const CHECKPOINT_FORMAT_VERSION: u32 = 1;
pub const INCREMENTAL_CHECKPOINT_FORMAT_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckpointFileEntry {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) checksum: u32,
}

#[derive(Debug, Clone)]
pub struct CheckpointInfo {
    pub checkpoint_id: u64,
    pub lsn: Lsn,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointStats {
    pub checkpoint_id: u64,
    pub data_flushed: u64,
    pub wal_truncated: u64,
    pub duration: Duration,
    pub snapshot_created: bool,
    pub checkpoint_seq: u64,
    pub data_files_created: usize,
    pub bytes_flushed: u64,
    pub wal_files_truncated: usize,
    pub trigger_reason: CheckpointTriggerReason,
}

#[derive(Debug, Clone)]
pub struct CheckpointData {
    pub vertex_count: u64,
    pub edge_count: u64,
    pub data_size: u64,
}

impl Default for CheckpointStrategy {
    fn default() -> Self {
        Self::Full
    }
}

impl crate::engine::persistence_coordinator::PersistenceCoordinator {
    pub(crate) fn cleanup_temporary_checkpoints(checkpoint_dir: &Path) -> StorageResult<()> {
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

    pub(crate) fn latest_published_sequence(
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
    pub(crate) fn cleanup_unpublished_checkpoints(
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

    pub(crate) fn current_lsn(&self) -> Lsn {
        match &self.wal_manager {
            Some(wal) => wal.read().current_lsn(),
            None => Lsn::ZERO,
        }
    }

    pub(crate) fn wal_bytes_since(&self, base_lsn: Lsn) -> u64 {
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

    pub fn checkpoint_trigger_reason(&self) -> Option<CheckpointTriggerReason> {
        let last_checkpoint_lsn = *self.last_checkpoint_lsn.read();
        let last_checkpoint = *self.last_checkpoint_time.read();
        let wal_bytes_since_checkpoint = self.wal_bytes_since(last_checkpoint_lsn);
        if wal_bytes_since_checkpoint >= self.config.max_wal_size
            || wal_bytes_since_checkpoint >= self.config.checkpoint_threshold
        {
            Some(CheckpointTriggerReason::WalSizeExceeded)
        } else if last_checkpoint.elapsed() >= self.config.auto_checkpoint_interval {
            Some(CheckpointTriggerReason::TimeSinceLastCheckpoint)
        } else {
            None
        }
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

    #[cfg(test)]
    pub(crate) fn create_checkpoint(
        &self,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
    ) -> StorageResult<CheckpointStats> {
        self.create_checkpoint_with_reason(flush_data, timestamp, CheckpointTriggerReason::Explicit)
    }

    pub fn create_checkpoint_with_reason(
        &self,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
        reason: CheckpointTriggerReason,
    ) -> StorageResult<CheckpointStats> {
        let result = self.create_checkpoint_inner(flush_data, timestamp, reason);
        match &result {
            Ok(_) => *self.last_checkpoint_error.write() = None,
            Err(error) => *self.last_checkpoint_error.write() = Some(error.to_string()),
        }
        result
    }

    pub(crate) fn create_checkpoint_with_guard(
        &self,
        guard: crate::engine::persistence_coordinator::PersistenceStateGuard,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
        reason: CheckpointTriggerReason,
    ) -> StorageResult<CheckpointStats> {
        let result = self.create_checkpoint_inner_with_guard(guard, flush_data, timestamp, reason);
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
        trigger_reason: CheckpointTriggerReason,
    ) -> StorageResult<CheckpointStats> {
        let guard = self
            .enter_state(crate::engine::persistence_coordinator::PersistenceState::Checkpointing)?;
        self.create_checkpoint_inner_with_guard(guard, flush_data, timestamp, trigger_reason)
    }

    fn create_checkpoint_inner_with_guard(
        &self,
        _guard: crate::engine::persistence_coordinator::PersistenceStateGuard,
        flush_data: impl FnOnce(&Path, Timestamp) -> StorageResult<CheckpointData>,
        timestamp: Timestamp,
        trigger_reason: CheckpointTriggerReason,
    ) -> StorageResult<CheckpointStats> {
        let start = Instant::now();

        let wal_lsn = {
            match &self.wal_manager {
                Some(wal) => {
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
                graphdb_core::StorageError::db_error(format!("Failed to create checkpoint: {}", e))
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

            std::fs::remove_dir_all(&checkpoint_dir)?;
            Self::sync_directory(&self.config.checkpoint_dir)?;
        }
        std::fs::create_dir(&temporary_dir)?;

        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::CheckpointRedoBefore,
        )?;
        let data = flush_data(&temporary_dir, timestamp)?;
        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::CheckpointIntentMid,
        )?;
        let files = Self::collect_checkpoint_files(&temporary_dir)?;
        // Check if incremental checkpoint meta was produced by flush (Shadow Page CoW)
        let incremental_meta_path = temporary_dir.join("incremental.meta");
        if incremental_meta_path.exists() {
            if let Ok(bytes) = std::fs::read(&incremental_meta_path) {
                if let Ok(meta) = serde_json::from_slice::<IncrementalCheckpointMeta>(&bytes) {
                    self.save_checkpoint_metadata_extended(
                        &temporary_dir,
                        &checkpoint,
                        &data,
                        &files,
                        Some(&meta),
                    )?;
                    // Also keep incremental.meta file for load_incremental_checkpoint_metadata (already there)
                } else {
                    self.save_checkpoint_metadata(&temporary_dir, &checkpoint, &data, &files)?;
                }
            } else {
                self.save_checkpoint_metadata(&temporary_dir, &checkpoint, &data, &files)?;
            }
        } else {
            self.save_checkpoint_metadata(&temporary_dir, &checkpoint, &data, &files)?;
        }
        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::CheckpointCommitMid,
        )?;
        Self::sync_tree(&temporary_dir)?;
        std::fs::rename(&temporary_dir, &checkpoint_dir)?;
        Self::sync_directory(&self.config.checkpoint_dir)?;
        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::CheckpointFsyncAfter,
        )?;

        let snapshot_created = if self.should_snapshot() {
            *self.state.write() =
                crate::engine::persistence_coordinator::PersistenceState::Snapshotting;
            if let Some(ref snapshot_manager) = self.snapshot_manager {
                let snapshot_options = SnapshotOptions::default();
                match snapshot_manager.create_snapshot(
                    crate::engine::snapshot_manager::CreateSnapshotParams {
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

        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::CheckpointVisibilityPublish,
        )?;
        self.publish_checkpoint_manifest(&checkpoint, &data, &checkpoint_dir, wal_lsn)?;

        {
            let mut cm = self.checkpoint_manager.write();
            cm.publish_checkpoint(&checkpoint).map_err(|e| {
                StorageError::db_error(format!("Failed to publish checkpoint: {}", e))
            })?;
        }

        if let Some(ref wal) = self.wal_manager {
            wal.read().set_checkpoint_seq(checkpoint.seq)?;
        }

        self.mark_checkpointed(wal_lsn);

        let safe_wal_lsn = if let Some(ref wal) = self.wal_manager {
            let manifest_safe_lsn = self.manifest_manager.latest_safe_lsn().map_err(|error| {
                StorageError::db_error(format!("Failed to get safe LSN: {}", error))
            })?;
            let outbox_safe_lsn = self
                .outbox_frontier_provider
                .read()
                .as_ref()
                .map(|provider| provider())
                .transpose()?
                .flatten()
                .map(|lsn| lsn.get());
            let safe_lsn = outbox_safe_lsn.map_or(manifest_safe_lsn.get(), |outbox| {
                manifest_safe_lsn.get().min(outbox)
            });
            let safe_wal_lsn = Lsn::new(safe_lsn);
            wal.read().truncate(safe_wal_lsn)?;
            safe_wal_lsn
        } else {
            wal_lsn
        };

        let stats = CheckpointStats {
            checkpoint_id: checkpoint.seq,
            data_flushed: data.data_size,
            wal_truncated: safe_wal_lsn.into(),
            duration: start.elapsed(),
            snapshot_created,
            checkpoint_seq: checkpoint.seq,
            data_files_created: files.len(),
            bytes_flushed: data.data_size,
            wal_files_truncated: if safe_wal_lsn != Lsn::ZERO { 1 } else { 0 },
            trigger_reason,
        };
        *self.last_checkpoint_stats.write() = Some(stats.clone());

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
        checkpoint: &graphdb_transaction::wal::Checkpoint,
        data: &CheckpointData,
        files: &[CheckpointFileEntry],
    ) -> StorageResult<()> {
        self.save_checkpoint_metadata_extended(dir, checkpoint, data, files, None)
    }

    fn save_checkpoint_metadata_extended(
        &self,
        dir: &Path,
        checkpoint: &graphdb_transaction::wal::Checkpoint,
        data: &CheckpointData,
        files: &[CheckpointFileEntry],
        incremental: Option<&IncrementalCheckpointMeta>,
    ) -> StorageResult<()> {
        use std::fs::File;
        use std::io::Write;

        let metadata_path = dir.join("checkpoint.meta");
        let mut file = File::create(metadata_path)?;

        let version = if incremental.is_some() {
            INCREMENTAL_CHECKPOINT_FORMAT_VERSION
        } else {
            CHECKPOINT_FORMAT_VERSION
        };
        writeln!(file, "format_version={}", version)?;
        writeln!(file, "checkpoint_id={}", checkpoint.seq)?;
        writeln!(file, "timestamp={}", checkpoint.timestamp)?;
        writeln!(file, "wal_lsn={}", checkpoint.lsn.0)?;
        writeln!(file, "vertex_count={}", data.vertex_count)?;
        writeln!(file, "edge_count={}", data.edge_count)?;
        writeln!(file, "data_size={}", data.data_size)?;
        writeln!(file, "created_at={:?}", SystemTime::now())?;
        if let Some(meta) = incremental {
            writeln!(file, "strategy={}", meta.strategy.as_str())?;
            if let Some(base) = meta.base_checkpoint_id {
                writeln!(file, "base_checkpoint_id={}", base)?;
            }
            writeln!(file, "dirty_pages={}", meta.dirty_pages.len())?;
            for page in &meta.dirty_pages {
                writeln!(
                    file,
                    "dirty_page={}:{}",
                    page.component.as_str(),
                    page.page_id
                )?;
            }
            for (page, checksum) in &meta.page_checksums {
                writeln!(
                    file,
                    "page_checksum={}:{}:{}",
                    page.component.as_str(),
                    page.page_id,
                    checksum
                )?;
            }
            writeln!(file, "total_pages={}", meta.total_pages)?;
            writeln!(file, "dirty_ratio={}", meta.dirty_ratio)?;
        } else {
            writeln!(file, "strategy=full")?;
        }
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



    pub(crate) fn collect_checkpoint_files(root: &Path) -> StorageResult<Vec<CheckpointFileEntry>> {
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

    pub(crate) fn sync_tree(root: &Path) -> StorageResult<()> {
        fn visit(directory: &Path) -> StorageResult<()> {
            for item in std::fs::read_dir(directory)? {
                let path = item?.path();
                if path.is_dir() {
                    visit(&path)?;
                } else if path.is_file() {
                    std::fs::File::open(path)?.sync_all()?;
                }
            }
            crate::engine::persistence_coordinator::PersistenceCoordinator::sync_directory(
                directory,
            )
        }

        visit(root)
    }

    pub(crate) fn sync_directory(directory: &Path) -> StorageResult<()> {
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
        self.fail_if_injected(
            crate::engine::persistence_coordinator::PersistenceFaultPoint::RecoveryScan,
        )?;
        let checkpoints_dir = &self.config.checkpoint_dir;

        if !checkpoints_dir.exists() {
            return Ok(None);
        }

        let published_manifest = self
            .manifest_manager
            .load_latest()
            .map_err(StorageError::db_error)?;
        let Some(published_manifest) = published_manifest else {
            let manifest_dir = self.config.checkpoint_dir.join("manifests");
            if manifest_dir.exists() {
                if let Ok(mut entries) = std::fs::read_dir(&manifest_dir) {
                    if entries.any(|e| e.is_ok()) {
                        return Err(StorageError::deserialize_error(
                            "Published manifest exists but failed validation. \
                             Checkpoint files or manifests may be corrupted."
                                .to_string(),
                        ));
                    }
                }
            }
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
        let mut timestamp: Option<Timestamp> = None;
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

        if !matches!(
            format_version,
            Some(CHECKPOINT_FORMAT_VERSION) | Some(INCREMENTAL_CHECKPOINT_FORMAT_VERSION)
        ) {
            // For backward compatibility, allow missing format_version to be treated as 1
            // but if present and unsupported, error.
            if format_version.is_some() {
                return Err(StorageError::deserialize_error(format!(
                    "Unsupported checkpoint format version: {:?}",
                    format_version
                )));
            }
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




    /// Publish a combined checkpoint manifest that atomically references the
    /// storage snapshot, outbox snapshot (if provided), and index manifests.
    fn publish_checkpoint_manifest(
        &self,
        checkpoint: &graphdb_transaction::wal::Checkpoint,
        data: &CheckpointData,
        checkpoint_dir: &Path,
        wal_lsn: Lsn,
    ) -> StorageResult<()> {
        let storage_snapshot_ref = CheckpointManifest::storage_snapshot_from_directory(
            checkpoint_dir,
            checkpoint.seq,
            data.vertex_count,
            data.edge_count,
        )
        .map_err(|error| {
            StorageError::db_error(format!(
                "Failed to build storage snapshot reference: {}",
                error
            ))
        })?;

        let storage_lsn = graphdb_core::types::CommitLsn::new(wal_lsn.into());
        let work_dir = self
            .config
            .data_dir
            .parent()
            .unwrap_or(&self.config.data_dir);
        let outbox_snapshot = graphdb_sync::find_latest_snapshot_at_or_before(
            &work_dir.join("outbox_snapshots"),
            storage_lsn.get(),
        )
        .map(|snapshot| CheckpointManifest::outbox_snapshot_from(&snapshot));
        let index_manifests = Self::collect_index_manifest_refs(checkpoint_dir)?;

        let outbox_enabled = work_dir.join("outbox/outbox.sqlite").exists()
            || work_dir.join("outbox_snapshots").is_dir();

        let manifest = if outbox_enabled {
            CheckpointManifest::new_with_outbox(
                checkpoint.seq,
                storage_lsn,
                storage_snapshot_ref,
                outbox_snapshot,
                index_manifests,
            )
            .map_err(StorageError::db_error)?
        } else {
            CheckpointManifest::new(
                checkpoint.seq,
                storage_lsn,
                storage_snapshot_ref,
                outbox_snapshot,
                index_manifests,
            )
            .map_err(StorageError::db_error)?
        };

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
                let manifest = crate::index::manifest::IndexManifest::load(&path)?;
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
}

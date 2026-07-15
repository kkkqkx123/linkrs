//! Snapshot Manager
//!
//! Provides multi-version snapshot support for crash recovery and point-in-time recovery.
//!
//! ## Features
//!
//! - Create immutable snapshots at any point in time
//! - Restore database to any snapshot version
//! - Automatic snapshot cleanup based on retention policy
//! - Efficient incremental snapshots using hard links (when supported)
//!
//! ## Snapshot Directory Structure
//!
//! ```text
//! snapshots/
//! ├── VERSION                    # Current snapshot format version
//! ├── metadata.json              # Snapshot metadata index
//! ├── snapshot_0000000001/       # Snapshot at timestamp 1
//! │   ├── meta.json
//! │   ├── schema.json
//! │   ├── vertices/
//! │   └── edges/
//! ├── snapshot_0000123456/       # Snapshot at timestamp 123456
//! │   ├── meta.json
//! │   ├── schema.json
//! │   ├── vertices/
//! │   └── edges/
//! └── ...
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::core::{StorageError, StorageResult};

/// Snapshot format version
const SNAPSHOT_FORMAT_VERSION: u32 = 1;

/// Snapshot metadata file name
const SNAPSHOT_META_FILE: &str = "meta.json";

/// Version file name
const VERSION_FILE: &str = "VERSION";

/// Metadata index file name
const METADATA_INDEX_FILE: &str = "metadata.json";

/// Snapshot information
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// Unique snapshot ID (timestamp)
    pub id: u64,
    /// Snapshot creation timestamp (Unix timestamp)
    pub created_at: u64,
    /// Snapshot name (optional)
    pub name: Option<String>,
    /// Description (optional)
    pub description: Option<String>,
    /// Size in bytes
    pub size_bytes: u64,
    /// Number of vertices
    pub vertex_count: u64,
    /// Number of edges
    pub edge_count: u64,
    /// Checkpoint sequence number
    pub checkpoint_seq: u64,
    /// WAL LSN at snapshot time
    pub wal_lsn: u64,
    /// Whether this is an incremental snapshot
    pub is_incremental: bool,
    /// Parent snapshot ID (for incremental snapshots)
    pub parent_id: Option<u64>,
}

/// Snapshot creation options
#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    /// Snapshot name
    pub name: Option<String>,
    /// Snapshot description
    pub description: Option<String>,
    /// Sync to disk after creation
    pub sync: bool,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            name: None,
            description: None,
            sync: true,
        }
    }
}

/// Snapshot retention policy
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Maximum number of snapshots to keep
    pub max_snapshots: usize,
    /// Maximum age of snapshots in seconds (0 = no limit)
    pub max_age_seconds: u64,
    /// Minimum time between snapshots in seconds
    pub min_interval_seconds: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            max_snapshots: 10,
            max_age_seconds: 7 * 24 * 3600, // 7 days
            min_interval_seconds: 60,       // 1 minute
        }
    }
}

/// Snapshot metadata index
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
struct SnapshotMetadataIndex {
    /// Format version
    version: u32,
    /// All snapshots indexed by ID
    snapshots: BTreeMap<u64, SnapshotInfo>,
    /// Current snapshot ID (latest successful)
    current_id: u64,
}

/// Snapshot Manager
///
/// Manages database snapshots for crash recovery and point-in-time recovery.
pub struct SnapshotManager {
    /// Snapshots directory
    snapshots_dir: PathBuf,
    /// Work directory (for temporary files)
    work_dir: PathBuf,
    /// Retention policy
    retention_policy: RetentionPolicy,
    /// Metadata index (cached)
    metadata_index: RwLock<SnapshotMetadataIndex>,
    /// Last snapshot creation time
    last_snapshot_time: RwLock<Option<SystemTime>>,
}

/// Parameters for creating a snapshot
pub struct CreateSnapshotParams {
    /// Immutable checkpoint directory to snapshot.
    pub source_dir: PathBuf,
    /// Unique snapshot ID
    pub snapshot_id: u64,
    /// Number of vertices
    pub vertex_count: u64,
    /// Number of edges
    pub edge_count: u64,
    /// Checkpoint sequence number
    pub checkpoint_seq: u64,
    /// WAL LSN at snapshot time
    pub wal_lsn: u64,
    /// Snapshot options
    pub options: SnapshotOptions,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    pub fn new<P: AsRef<Path>>(snapshots_dir: P, work_dir: P) -> StorageResult<Self> {
        let snapshots_dir = snapshots_dir.as_ref().to_path_buf();
        let work_dir = work_dir.as_ref().to_path_buf();

        fs::create_dir_all(&snapshots_dir).map_err(|e| {
            StorageError::io_error(format!("Failed to create snapshots dir: {}", e))
        })?;
        fs::create_dir_all(&work_dir)
            .map_err(|e| StorageError::io_error(format!("Failed to create work dir: {}", e)))?;

        let mut manager = Self {
            snapshots_dir,
            work_dir,
            retention_policy: RetentionPolicy::default(),
            metadata_index: RwLock::new(SnapshotMetadataIndex::default()),
            last_snapshot_time: RwLock::new(None),
        };

        manager.init()?;

        Ok(manager)
    }

    /// Initialize snapshot manager
    fn init(&mut self) -> StorageResult<()> {
        let version_path = self.snapshots_dir.join(VERSION_FILE);
        if version_path.exists() {
            let version = fs::read_to_string(&version_path)?
                .trim()
                .parse::<u32>()
                .map_err(|error| {
                    StorageError::deserialize_error(format!(
                        "Invalid snapshot format version: {}",
                        error
                    ))
                })?;
            if version != SNAPSHOT_FORMAT_VERSION {
                return Err(StorageError::deserialize_error(format!(
                    "Unsupported snapshot format version {}",
                    version
                )));
            }
        } else {
            self.write_version_file()?;
        }

        self.cleanup_deleting_directories()?;
        self.load_metadata_index()?;

        Ok(())
    }

    fn cleanup_deleting_directories(&self) -> StorageResult<()> {
        for entry in fs::read_dir(&self.snapshots_dir)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with(".snapshot_deleting_") && path.is_dir() {
                fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }

    /// Write version file
    fn write_version_file(&self) -> StorageResult<()> {
        let version_path = self.snapshots_dir.join(VERSION_FILE);
        fs::write(&version_path, SNAPSHOT_FORMAT_VERSION.to_string())
            .map_err(|e| StorageError::io_error(format!("Failed to write version file: {}", e)))?;
        Ok(())
    }

    /// Load metadata index from disk
    fn load_metadata_index(&self) -> StorageResult<()> {
        let index_path = self.snapshots_dir.join(METADATA_INDEX_FILE);
        let indexed = if index_path.exists() {
            let content = fs::read_to_string(&index_path).map_err(|e| {
                StorageError::io_error(format!("Failed to read metadata index: {}", e))
            })?;
            let index: SnapshotMetadataIndex = serde_json::from_str(&content).map_err(|e| {
                StorageError::deserialize_error(format!("Invalid metadata index: {}", e))
            })?;
            if index.version != 0 && index.version != SNAPSHOT_FORMAT_VERSION {
                return Err(StorageError::deserialize_error(format!(
                    "Unsupported snapshot metadata version {}",
                    index.version
                )));
            }
            index
        } else {
            SnapshotMetadataIndex::default()
        };

        // Always reconcile the index with immutable snapshot directories. This
        // closes the crash window between directory publication and index save.
        let mut snapshots = BTreeMap::new();
        for entry in fs::read_dir(&self.snapshots_dir)? {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if !path.is_dir() || !name.starts_with("snapshot_") {
                continue;
            }
            let meta_path = path.join(SNAPSHOT_META_FILE);
            if !meta_path.is_file() {
                continue;
            }
            let meta = fs::read_to_string(&meta_path)?;
            let info: SnapshotInfo = serde_json::from_str(&meta).map_err(|error| {
                StorageError::deserialize_error(format!(
                    "Invalid snapshot metadata in {}: {}",
                    path.display(),
                    error
                ))
            })?;
            snapshots.insert(info.id, info);
        }

        let reconciled = SnapshotMetadataIndex {
            version: SNAPSHOT_FORMAT_VERSION,
            current_id: (indexed.current_id != 0)
                .then_some(indexed.current_id)
                .filter(|id| snapshots.contains_key(id))
                .or_else(|| snapshots.keys().next_back().copied())
                .unwrap_or(0),
            snapshots,
        };
        let changed = indexed.version != reconciled.version
            || indexed.current_id != reconciled.current_id
            || indexed.snapshots != reconciled.snapshots;
        *self.metadata_index.write() = reconciled;
        if changed || !index_path.exists() {
            self.save_metadata_index()?;
        }

        Ok(())
    }

    /// Save metadata index to disk
    fn save_metadata_index(&self) -> StorageResult<()> {
        let index_path = self.snapshots_dir.join(METADATA_INDEX_FILE);

        let index = self.metadata_index.read();
        let content = serde_json::to_string_pretty(&*index).map_err(|e| {
            StorageError::serialize_error(format!("Failed to serialize metadata: {}", e))
        })?;

        let temporary = index_path.with_extension("json.tmp");
        fs::write(&temporary, content).map_err(|e| {
            StorageError::io_error(format!("Failed to write metadata index: {}", e))
        })?;
        fs::File::open(&temporary)?.sync_all()?;
        fs::rename(&temporary, &index_path)?;
        fs::File::open(&self.snapshots_dir)?.sync_all()?;

        Ok(())
    }

    /// Get snapshot directory path
    fn get_snapshot_dir(&self, snapshot_id: u64) -> PathBuf {
        self.snapshots_dir
            .join(format!("snapshot_{:010}", snapshot_id))
    }

    /// Create a new snapshot
    ///
    /// This copies all data files to a new snapshot directory.
    pub fn create_snapshot(&self, params: CreateSnapshotParams) -> StorageResult<SnapshotInfo> {
        let now = SystemTime::now();

        if !params.source_dir.is_dir() {
            return Err(StorageError::not_found(format!(
                "Snapshot source directory does not exist: {}",
                params.source_dir.display()
            )));
        }

        if let Some(last_time) = *self.last_snapshot_time.read() {
            let elapsed = now
                .duration_since(last_time)
                .unwrap_or(Duration::from_secs(0));
            if elapsed.as_secs() < self.retention_policy.min_interval_seconds {
                return Err(StorageError::invalid_operation(format!(
                    "Snapshot too frequent, min interval is {} seconds",
                    self.retention_policy.min_interval_seconds
                )));
            }
        }

        let snapshot_dir = self.get_snapshot_dir(params.snapshot_id);

        if snapshot_dir.exists() {
            return Err(StorageError::already_exists(format!(
                "Snapshot {} already exists",
                params.snapshot_id
            )));
        }

        let temp_dir = self
            .work_dir
            .join(format!("snapshot_temp_{}", params.snapshot_id));
        if temp_dir.exists() {
            fs::remove_dir_all(&temp_dir)?;
        }
        fs::create_dir_all(&temp_dir)
            .map_err(|e| StorageError::io_error(format!("Failed to create temp dir: {}", e)))?;

        let info = match self.create_snapshot_internal(&params, &temp_dir, &temp_dir) {
            Ok(info) => info,
            Err(error) => {
                let _ = fs::remove_dir_all(&temp_dir);
                return Err(error);
            }
        };
        if params.options.sync {
            self.sync_directory(&temp_dir)?;
        }
        fs::rename(&temp_dir, &snapshot_dir).map_err(|error| {
            if error.raw_os_error() == Some(18) {
                StorageError::io_error(
                    "Snapshot work directory and snapshot directory must be on the same filesystem",
                )
            } else {
                StorageError::io_error(format!("Failed to publish snapshot: {}", error))
            }
        })?;
        fs::File::open(&self.snapshots_dir)?.sync_all()?;

        {
            let mut index = self.metadata_index.write();
            index.snapshots.insert(params.snapshot_id, info.clone());
            index.current_id = params.snapshot_id;
            index.version = SNAPSHOT_FORMAT_VERSION;
        }
        self.save_metadata_index()?;

        *self.last_snapshot_time.write() = Some(now);

        self.cleanup_old_snapshots()?;

        Ok(info)
    }

    fn create_snapshot_internal(
        &self,
        params: &CreateSnapshotParams,
        snapshot_dir: &Path,
        _temp_dir: &Path,
    ) -> StorageResult<SnapshotInfo> {
        let mut size_bytes = 0u64;

        self.copy_directory(&params.source_dir, snapshot_dir, &mut size_bytes)?;

        let info = SnapshotInfo {
            id: params.snapshot_id,
            created_at: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            name: params.options.name.clone(),
            description: params.options.description.clone(),
            size_bytes,
            vertex_count: params.vertex_count,
            edge_count: params.edge_count,
            checkpoint_seq: params.checkpoint_seq,
            wal_lsn: params.wal_lsn,
            is_incremental: false,
            parent_id: None,
        };

        let meta_path = snapshot_dir.join(SNAPSHOT_META_FILE);
        let meta_content = serde_json::to_string_pretty(&info).map_err(|e| {
            StorageError::serialize_error(format!("Failed to serialize snapshot meta: {}", e))
        })?;
        fs::write(&meta_path, meta_content)
            .map_err(|e| StorageError::io_error(format!("Failed to write snapshot meta: {}", e)))?;

        Ok(info)
    }

    fn copy_directory(&self, src: &Path, dst: &Path, total_size: &mut u64) -> StorageResult<()> {
        if !src.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(src)
            .map_err(|e| StorageError::io_error(format!("Failed to read directory: {}", e)))?
        {
            let entry = entry
                .map_err(|e| StorageError::io_error(format!("Failed to read entry: {}", e)))?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            let file_type = entry
                .file_type()
                .map_err(|e| StorageError::io_error(format!("Failed to get file type: {}", e)))?;

            if file_type.is_dir() {
                fs::create_dir_all(&dst_path).map_err(|e| {
                    StorageError::io_error(format!("Failed to create directory: {}", e))
                })?;
                self.copy_directory(&src_path, &dst_path, total_size)?;
            } else if file_type.is_file() {
                fs::copy(&src_path, &dst_path)
                    .map_err(|e| StorageError::io_error(format!("Failed to copy file: {}", e)))?;

                if let Ok(metadata) = fs::metadata(&src_path) {
                    *total_size += metadata.len();
                }
            }
        }

        Ok(())
    }

    fn sync_directory(&self, dir: &Path) -> StorageResult<()> {
        for entry in fs::read_dir(dir).map_err(|e| {
            StorageError::io_error(format!("Failed to read directory for sync: {}", e))
        })? {
            let entry = entry
                .map_err(|e| StorageError::io_error(format!("Failed to read entry: {}", e)))?;
            let path = entry.path();

            let file_type = entry
                .file_type()
                .map_err(|e| StorageError::io_error(format!("Failed to get file type: {}", e)))?;

            if file_type.is_dir() {
                self.sync_directory(&path)?;
            } else if file_type.is_file() {
                let file = fs::File::open(&path).map_err(|e| {
                    StorageError::io_error(format!("Failed to open file for sync: {}", e))
                })?;
                file.sync_all()
                    .map_err(|e| StorageError::io_error(format!("Failed to sync file: {}", e)))?;
            }
        }

        fs::File::open(dir)
            .map_err(|e| {
                StorageError::io_error(format!("Failed to open directory for sync: {}", e))
            })?
            .sync_all()
            .map_err(|e| StorageError::io_error(format!("Failed to sync directory: {}", e)))?;

        Ok(())
    }

    /// Get snapshot info by ID
    pub fn get_snapshot(&self, snapshot_id: u64) -> Option<SnapshotInfo> {
        self.metadata_index
            .read()
            .snapshots
            .get(&snapshot_id)
            .cloned()
    }

    /// Get the latest snapshot
    pub fn get_latest_snapshot(&self) -> Option<SnapshotInfo> {
        let index = self.metadata_index.read();
        if index.current_id > 0 {
            index.snapshots.get(&index.current_id).cloned()
        } else {
            index.snapshots.values().last().cloned()
        }
    }

    /// Delete a snapshot
    pub fn delete_snapshot(&self, snapshot_id: u64) -> StorageResult<()> {
        if !self
            .metadata_index
            .read()
            .snapshots
            .contains_key(&snapshot_id)
        {
            return Err(StorageError::not_found(format!(
                "Snapshot {} not found",
                snapshot_id
            )));
        }

        let snapshot_dir = self.get_snapshot_dir(snapshot_id);
        let deleting_dir = self
            .snapshots_dir
            .join(format!(".snapshot_deleting_{}", snapshot_id));
        if snapshot_dir.exists() {
            fs::rename(&snapshot_dir, &deleting_dir).map_err(|e| {
                StorageError::io_error(format!("Failed to stage snapshot deletion: {}", e))
            })?;
        }

        let mut index = self.metadata_index.write();
        index.snapshots.remove(&snapshot_id);

        if index.current_id == snapshot_id {
            index.current_id = index.snapshots.keys().last().copied().unwrap_or(0);
        }

        drop(index);

        self.save_metadata_index()?;

        if deleting_dir.exists() {
            fs::remove_dir_all(&deleting_dir).map_err(|e| {
                StorageError::io_error(format!("Failed to delete snapshot dir: {}", e))
            })?;
        }
        fs::File::open(&self.snapshots_dir)?.sync_all()?;

        Ok(())
    }

    /// Clean up old snapshots based on retention policy
    pub fn cleanup_old_snapshots(&self) -> StorageResult<usize> {
        let mut deleted_count = 0;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut to_delete: Vec<u64> = Vec::new();

        {
            let index = self.metadata_index.read();
            let snapshots: Vec<_> = index.snapshots.iter().collect();

            for (id, info) in snapshots.iter() {
                if self.retention_policy.max_age_seconds > 0
                    && now - info.created_at > self.retention_policy.max_age_seconds
                    && **id != index.current_id
                {
                    to_delete.push(**id);
                }
            }

            let keep = self.retention_policy.max_snapshots.max(1);
            let excess_count = snapshots.len().saturating_sub(keep);
            if to_delete.len() < excess_count {
                let mut sorted_ids: Vec<_> = index.snapshots.keys().copied().collect();
                sorted_ids.sort();

                for &id in sorted_ids.iter().take(excess_count - to_delete.len()) {
                    if id != index.current_id && !to_delete.contains(&id) {
                        to_delete.push(id);
                    }
                }
            }
        }

        for id in to_delete {
            self.delete_snapshot(id)?;
            deleted_count += 1;
        }

        Ok(deleted_count)
    }

    /// Get total size of all snapshots
    pub fn total_snapshot_size(&self) -> u64 {
        self.metadata_index
            .read()
            .snapshots
            .values()
            .map(|info| info.size_bytes)
            .sum()
    }

    /// Get snapshot count
    pub fn snapshot_count(&self) -> usize {
        self.metadata_index.read().snapshots.len()
    }

    /// Checkpoint sequences that remain referenced by published snapshots.
    pub fn retained_checkpoint_sequences(&self) -> std::collections::BTreeSet<u64> {
        self.metadata_index
            .read()
            .snapshots
            .values()
            .map(|snapshot| snapshot.checkpoint_seq)
            .collect()
    }

    /// Verify snapshot integrity
    pub fn verify_snapshot(&self, snapshot_id: u64) -> StorageResult<bool> {
        let info = self.get_snapshot(snapshot_id).ok_or_else(|| {
            StorageError::not_found(format!("Snapshot {} not found", snapshot_id))
        })?;

        let snapshot_dir = self.get_snapshot_dir(snapshot_id);

        if !snapshot_dir.exists() {
            return Ok(false);
        }

        let meta_path = snapshot_dir.join(SNAPSHOT_META_FILE);
        if !meta_path.exists() {
            return Ok(false);
        }

        let content = fs::read_to_string(&meta_path)
            .map_err(|e| StorageError::io_error(format!("Failed to read snapshot meta: {}", e)))?;

        let loaded_info: SnapshotInfo = serde_json::from_str(&content).map_err(|e| {
            StorageError::deserialize_error(format!("Invalid snapshot meta: {}", e))
        })?;
        if loaded_info.id != info.id
            || loaded_info.checkpoint_seq != info.checkpoint_seq
            || loaded_info.wal_lsn != info.wal_lsn
        {
            return Ok(false);
        }

        let checkpoint_meta = snapshot_dir.join("checkpoint.meta");
        if checkpoint_meta.exists() {
            let content = fs::read_to_string(checkpoint_meta)?;
            for line in content.lines().filter(|line| line.starts_with("file=")) {
                let fields: Vec<_> = line[5..].rsplitn(3, '|').collect();
                if fields.len() != 3 {
                    return Ok(false);
                }
                let checksum = match fields[0].parse::<u32>() {
                    Ok(value) => value,
                    Err(_) => return Ok(false),
                };
                let size = match fields[1].parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => return Ok(false),
                };
                let relative = PathBuf::from(fields[2]);
                if relative.is_absolute()
                    || relative
                        .components()
                        .any(|component| component == std::path::Component::ParentDir)
                {
                    return Ok(false);
                }
                let path = snapshot_dir.join(relative);
                let bytes = match fs::read(path) {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(false),
                };
                if bytes.len() as u64 != size || crc32fast::hash(&bytes) != checksum {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_snapshot_manager_creation() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");

        let manager = SnapshotManager::new(&snapshots_dir, &work_dir);
        assert!(manager.is_ok());

        let manager = manager.unwrap();
        assert_eq!(manager.snapshot_count(), 0);
    }

    #[test]
    fn test_create_and_list_snapshots() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");

        fs::create_dir_all(&data_dir).expect("Failed to create data dir");
        fs::write(data_dir.join("test.txt"), "test data").expect("Failed to write test file");

        let manager =
            SnapshotManager::new(&snapshots_dir, &work_dir).expect("Failed to create manager");

        let info = manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir.to_path_buf(),
                snapshot_id: 1,
                vertex_count: 100,
                edge_count: 50,
                checkpoint_seq: 1,
                wal_lsn: 1000,
                options: SnapshotOptions::default(),
            })
            .expect("Failed to create snapshot");

        assert_eq!(info.id, 1);
        assert_eq!(info.vertex_count, 100);
        assert_eq!(info.edge_count, 50);

        assert_eq!(manager.snapshot_count(), 1);
    }

    #[test]
    fn test_delete_snapshot() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");

        fs::create_dir_all(&data_dir).expect("Failed to create data dir");

        let manager =
            SnapshotManager::new(&snapshots_dir, &work_dir).expect("Failed to create manager");

        manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir.to_path_buf(),
                snapshot_id: 1,
                vertex_count: 100,
                edge_count: 50,
                checkpoint_seq: 1,
                wal_lsn: 1000,
                options: SnapshotOptions::default(),
            })
            .expect("Failed to create snapshot");

        assert_eq!(manager.snapshot_count(), 1);

        manager
            .delete_snapshot(1)
            .expect("Failed to delete snapshot");

        assert_eq!(manager.snapshot_count(), 0);
    }

    #[test]
    fn test_cleanup_old_snapshots() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");

        fs::create_dir_all(&data_dir).expect("Failed to create data dir");

        let mut manager =
            SnapshotManager::new(&snapshots_dir, &work_dir).expect("Failed to create manager");
        manager.retention_policy.min_interval_seconds = 0;

        for i in 1..=3 {
            manager
                .create_snapshot(CreateSnapshotParams {
                    source_dir: data_dir.to_path_buf(),
                    snapshot_id: i,
                    vertex_count: 100,
                    edge_count: 50,
                    checkpoint_seq: i,
                    wal_lsn: i * 1000,
                    options: SnapshotOptions::default(),
                })
                .expect("Failed to create snapshot");
        }

        assert_eq!(manager.snapshot_count(), 3);
    }

    #[test]
    fn missing_snapshot_source_does_not_publish_directory() {
        let temp_dir = TempDir::new().expect("temporary directory");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let manager = SnapshotManager::new(&snapshots_dir, &work_dir).expect("manager");
        let result = manager.create_snapshot(CreateSnapshotParams {
            source_dir: temp_dir.path().join("missing-checkpoint"),
            snapshot_id: 9,
            vertex_count: 0,
            edge_count: 0,
            checkpoint_seq: 9,
            wal_lsn: 0,
            options: SnapshotOptions::default(),
        });
        assert!(result.is_err());
        assert!(!snapshots_dir.join("snapshot_0000000009").exists());
    }

    #[test]
    fn metadata_index_is_rebuilt_from_published_snapshots() {
        let temp_dir = TempDir::new().expect("temporary directory");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");
        fs::create_dir_all(&data_dir).expect("data directory");
        fs::write(data_dir.join("table.data"), b"data").expect("data file");
        let manager = SnapshotManager::new(&snapshots_dir, &work_dir).expect("manager");
        manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir,
                snapshot_id: 3,
                vertex_count: 1,
                edge_count: 0,
                checkpoint_seq: 3,
                wal_lsn: 12,
                options: SnapshotOptions::default(),
            })
            .expect("snapshot should be created");
        fs::remove_file(snapshots_dir.join(METADATA_INDEX_FILE)).expect("index should remove");

        let reopened = SnapshotManager::new(&snapshots_dir, &work_dir).expect("reopen manager");
        assert_eq!(reopened.snapshot_count(), 1);
        assert_eq!(reopened.get_latest_snapshot().map(|info| info.id), Some(3));
    }

    #[test]
    fn startup_reconciles_stale_metadata_index() {
        let temp_dir = TempDir::new().expect("temporary directory");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");
        fs::create_dir_all(&data_dir).expect("data directory");
        let manager = SnapshotManager::new(&snapshots_dir, &work_dir).expect("manager");
        manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir,
                snapshot_id: 7,
                vertex_count: 1,
                edge_count: 0,
                checkpoint_seq: 7,
                wal_lsn: 70,
                options: SnapshotOptions::default(),
            })
            .expect("snapshot");
        let stale = SnapshotMetadataIndex {
            version: SNAPSHOT_FORMAT_VERSION,
            snapshots: BTreeMap::new(),
            current_id: 0,
        };
        fs::write(
            snapshots_dir.join(METADATA_INDEX_FILE),
            serde_json::to_string(&stale).expect("stale index serialization"),
        )
        .expect("stale index write");
        drop(manager);

        let reopened = SnapshotManager::new(&snapshots_dir, &work_dir).expect("reopen manager");
        assert_eq!(reopened.snapshot_count(), 1);
        assert_eq!(reopened.get_latest_snapshot().expect("latest").id, 7);
    }

    #[test]
    fn age_retention_runs_even_when_count_is_below_limit() {
        let temp_dir = TempDir::new().expect("temporary directory");
        let snapshots_dir = temp_dir.path().join("snapshots");
        let work_dir = temp_dir.path().join("work");
        let data_dir = temp_dir.path().join("data");
        fs::create_dir_all(&data_dir).expect("data directory");
        let mut manager = SnapshotManager::new(&snapshots_dir, &work_dir).expect("manager");
        manager.retention_policy.min_interval_seconds = 0;
        manager.retention_policy.max_snapshots = 10;
        manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir.clone(),
                snapshot_id: 1,
                vertex_count: 1,
                edge_count: 0,
                checkpoint_seq: 1,
                wal_lsn: 10,
                options: SnapshotOptions::default(),
            })
            .expect("first snapshot");
        manager
            .create_snapshot(CreateSnapshotParams {
                source_dir: data_dir,
                snapshot_id: 2,
                vertex_count: 1,
                edge_count: 0,
                checkpoint_seq: 2,
                wal_lsn: 20,
                options: SnapshotOptions::default(),
            })
            .expect("second snapshot");
        manager
            .metadata_index
            .write()
            .snapshots
            .get_mut(&1)
            .expect("first metadata")
            .created_at = 0;
        manager.retention_policy.max_age_seconds = 1;
        assert_eq!(manager.cleanup_old_snapshots().expect("cleanup"), 1);
        assert!(manager.get_snapshot(1).is_none());
        assert!(manager.get_snapshot(2).is_some());
    }
}

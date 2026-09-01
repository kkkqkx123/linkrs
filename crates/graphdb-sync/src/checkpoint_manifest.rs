//! Combined Checkpoint Manifest
//!
//! This module provides the atomic checkpoint manifest that ties together:
//! - Storage snapshot
//! - Outbox snapshot
//! - Native-index manifests
//!
//! The manifest is published atomically and serves as the authoritative source
//! for WAL cleanup decisions via the common safe LSN.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use graphdb_core::types::{CommitLsn, IndexGeneration};
use serde::{Deserialize, Serialize};

use crate::sqlite_outbox::OutboxSnapshot;

/// Combined checkpoint manifest that atomically references all snapshot components.
///
/// This is the core data structure. It ensures that:
/// 1. Storage snapshot, outbox snapshot, and index manifests are published together
/// 2. WAL cleanup uses the common safe LSN from this manifest
/// 3. Recovery can rebuild from any valid manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointManifest {
    /// Manifest format version for backward compatibility
    pub format_version: u32,
    /// Unique checkpoint identifier (monotonically increasing)
    pub checkpoint_id: u64,
    /// Wall-clock timestamp when checkpoint was created
    pub timestamp: u64,
    /// Storage snapshot LSN - the LSN of the last committed transaction included in storage
    pub storage_lsn: CommitLsn,
    /// Outbox snapshot LSN - the materialized LSN of the outbox snapshot
    pub outbox_lsn: CommitLsn,
    /// Whether the database has a durable SQLite outbox projection.
    ///
    /// This is distinct from `outbox_snapshot`: a checkpoint may be created
    /// while the outbox is temporarily unavailable. In that case WAL cannot
    /// be reclaimed past zero because the missing projection must be rebuilt
    /// from the retained WAL.
    pub outbox_enabled: bool,
    /// Common safe LSN - the minimum of storage_lsn and outbox_lsn
    /// This is the LSN up to which WAL can be safely truncated
    pub safe_lsn: CommitLsn,
    /// Reference to the storage snapshot
    pub storage_snapshot: StorageSnapshotRef,
    /// Reference to the outbox snapshot (if enabled)
    pub outbox_snapshot: Option<OutboxSnapshotRef>,
    /// References to native index manifests
    pub index_manifests: Vec<IndexManifestRef>,
    /// Highest commit timestamp that is fully incorporated in this checkpoint.
    /// Used by recovery to continue timestamp allocation after the checkpoint
    /// without reusing or skipping timestamps.
    #[serde(default)]
    pub max_commit_timestamp: u64,
    /// Schema catalog version at checkpoint time.
    #[serde(default)]
    pub schema_catalog_version: u64,
    /// Monotonic sequence (e.g. allocator) version at checkpoint time.
    #[serde(default)]
    pub sequence_version: u64,
    /// Checksum of the entire manifest for integrity verification
    pub manifest_checksum: u32,
}

/// Reference to a storage snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageSnapshotRef {
    /// Path to the storage snapshot directory
    pub path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// CRC32 checksum
    pub checksum: u32,
    /// Checksummed files contained by the snapshot directory.
    pub files: Vec<StorageFileRef>,
    /// Checkpoint sequence number
    pub checkpoint_seq: u64,
    /// Number of vertices
    pub vertex_count: u64,
    /// Number of edges
    pub edge_count: u64,
}

/// Immutable file reference belonging to a storage snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageFileRef {
    /// Path relative to the storage snapshot directory.
    pub path: PathBuf,
    /// Size in bytes.
    pub size_bytes: u64,
    /// CRC32 checksum.
    pub checksum: u32,
}

/// Reference to an outbox snapshot
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxSnapshotRef {
    /// Path to the outbox snapshot file
    pub path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// CRC32 checksum
    pub checksum: u32,
    /// Materialized LSN
    pub materialized_lsn: CommitLsn,
}

/// Reference to a native index manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexManifestRef {
    /// Logical namespace of the index.
    pub space_id: u64,
    /// Index identifier
    pub index_id: u64,
    /// Index generation
    pub generation: IndexGeneration,
    /// Path to the manifest file
    pub path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// CRC32 checksum
    pub checksum: u32,
}

impl CheckpointManifest {
    /// Current manifest format version
    pub const CURRENT_FORMAT_VERSION: u32 = 4;

    /// Build a storage reference from a fully materialized checkpoint
    /// directory. The file list is part of the combined manifest so recovery
    /// can reject a directory that was only partially written or corrupted.
    pub fn storage_snapshot_from_directory(
        path: impl AsRef<Path>,
        checkpoint_seq: u64,
        vertex_count: u64,
        edge_count: u64,
    ) -> Result<StorageSnapshotRef, String> {
        let path = path.as_ref();
        let files = collect_storage_files(path)?;
        let size_bytes = files.iter().map(|file| file.size_bytes).sum();
        let checksum = storage_files_checksum(&files)?;
        Ok(StorageSnapshotRef {
            path: path.to_path_buf(),
            size_bytes,
            checksum,
            files,
            checkpoint_seq,
            vertex_count,
            edge_count,
        })
    }

    /// Create a new checkpoint manifest from component snapshots.
    ///
    /// This computes the common safe LSN as the minimum of storage and outbox LSNs.
    /// When the outbox is enabled but no snapshot is available, safe_lsn is
    /// zero so the retained WAL can rebuild the missing projection.
    pub fn new(
        checkpoint_id: u64,
        storage_lsn: CommitLsn,
        storage_snapshot: StorageSnapshotRef,
        outbox_snapshot: Option<OutboxSnapshotRef>,
        index_manifests: Vec<IndexManifestRef>,
    ) -> Result<Self, String> {
        Self::new_with_outbox_state(
            checkpoint_id,
            storage_lsn,
            storage_snapshot,
            outbox_snapshot,
            index_manifests,
            false,
        )
    }

    /// Create a manifest for a database whose SQLite outbox is part of the
    /// durability boundary.
    pub fn new_with_outbox(
        checkpoint_id: u64,
        storage_lsn: CommitLsn,
        storage_snapshot: StorageSnapshotRef,
        outbox_snapshot: Option<OutboxSnapshotRef>,
        index_manifests: Vec<IndexManifestRef>,
    ) -> Result<Self, String> {
        Self::new_with_outbox_state(
            checkpoint_id,
            storage_lsn,
            storage_snapshot,
            outbox_snapshot,
            index_manifests,
            true,
        )
    }

    fn new_with_outbox_state(
        checkpoint_id: u64,
        storage_lsn: CommitLsn,
        storage_snapshot: StorageSnapshotRef,
        outbox_snapshot: Option<OutboxSnapshotRef>,
        index_manifests: Vec<IndexManifestRef>,
        outbox_enabled: bool,
    ) -> Result<Self, String> {
        let outbox_lsn = outbox_snapshot
            .as_ref()
            .map(|s| s.materialized_lsn)
            .unwrap_or(CommitLsn::ZERO);

        // A missing outbox snapshot is safe only when no outbox projection is
        // configured. If the projection is enabled, retaining WAL from zero
        // is the only boundary that permits a complete rebuild after restart.
        let safe_lsn = if outbox_enabled && outbox_snapshot.is_none() {
            CommitLsn::ZERO
        } else if outbox_snapshot.is_some() && outbox_lsn < storage_lsn {
            outbox_lsn
        } else {
            storage_lsn
        };

        let mut manifest = Self {
            format_version: Self::CURRENT_FORMAT_VERSION,
            checkpoint_id,
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            storage_lsn,
            outbox_lsn,
            outbox_enabled,
            safe_lsn,
            storage_snapshot,
            outbox_snapshot,
            index_manifests,
            max_commit_timestamp: storage_lsn.get(),
            schema_catalog_version: 0,
            sequence_version: 0,
            manifest_checksum: 0,
        };

        manifest.manifest_checksum = manifest.compute_checksum()?;
        Ok(manifest)
    }

    /// Compute checksum of the manifest (excluding the checksum field itself)
    fn compute_checksum(&self) -> Result<u32, String> {
        let mut clone = self.clone();
        clone.manifest_checksum = 0;
        let bytes = postcard::to_allocvec(&clone)
            .map_err(|error| format!("Failed to serialize checkpoint manifest: {error}"))?;
        Ok(crc32fast::hash(&bytes))
    }

    /// Verify the manifest checksum
    pub fn verify_checksum(&self) -> Result<bool, String> {
        let expected = self.manifest_checksum;
        let mut clone = self.clone();
        clone.manifest_checksum = 0;
        let bytes = postcard::to_allocvec(&clone)
            .map_err(|error| format!("Failed to serialize checkpoint manifest: {error}"))?;
        let computed = crc32fast::hash(&bytes);
        Ok(computed == expected)
    }

    /// Validate the manifest structure and references
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != Self::CURRENT_FORMAT_VERSION
            && self.format_version != Self::CURRENT_FORMAT_VERSION - 1
        {
            return Err(format!(
                "Unsupported manifest format version: {}",
                self.format_version
            ));
        }

        if !self.verify_checksum()? {
            return Err("Manifest checksum verification failed".to_string());
        }

        // Validate that safe_lsn is correctly computed
        let outbox_lsn = self
            .outbox_snapshot
            .as_ref()
            .map(|s| s.materialized_lsn)
            .unwrap_or(CommitLsn::ZERO);

        let expected_safe = if self.outbox_enabled && self.outbox_snapshot.is_none() {
            CommitLsn::ZERO
        } else if self.outbox_snapshot.is_some() && outbox_lsn < self.storage_lsn {
            outbox_lsn
        } else {
            self.storage_lsn
        };

        if self.safe_lsn != expected_safe {
            return Err(format!(
                "Invalid safe_lsn: expected {}, got {}",
                expected_safe, self.safe_lsn
            ));
        }

        if self.storage_snapshot.checkpoint_seq != self.checkpoint_id {
            return Err(format!(
                "Storage snapshot checkpoint sequence {} does not match manifest {}",
                self.storage_snapshot.checkpoint_seq, self.checkpoint_id
            ));
        }

        if self.outbox_snapshot.is_some() && self.outbox_lsn > self.storage_lsn {
            return Err(format!(
                "Outbox snapshot LSN {} is ahead of storage LSN {}",
                self.outbox_lsn, self.storage_lsn
            ));
        }

        // Validate that outbox LSN is consistent with outbox snapshot presence
        if self.outbox_snapshot.is_none() && self.outbox_lsn != CommitLsn::ZERO {
            return Err(
                "Outbox LSN is non-zero but outbox snapshot reference is missing".to_string(),
            );
        }

        verify_storage_reference(&self.storage_snapshot)?;
        if let Some(snapshot) = &self.outbox_snapshot {
            verify_file_reference(
                &snapshot.path,
                snapshot.size_bytes,
                snapshot.checksum,
                "outbox snapshot",
            )?;
        }
        for manifest in &self.index_manifests {
            verify_file_reference(
                &manifest.path,
                manifest.size_bytes,
                manifest.checksum,
                "index manifest",
            )?;
        }

        Ok(())
    }

    /// Load a manifest from a file
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes = std::fs::read(path.as_ref()).map_err(|error| error.to_string())?;
        let manifest: Self = postcard::from_bytes(&bytes).map_err(|error| error.to_string())?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Save the manifest to a temporary file, sync, then atomically rename
    pub fn save_atomic(&self, destination: impl AsRef<Path>) -> Result<(), String> {
        let destination = destination.as_ref();

        // Ensure parent directory exists
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        // Write to temporary file first
        let temporary = destination.with_extension("tmp");
        let bytes = postcard::to_allocvec(self).map_err(|error| error.to_string())?;
        std::fs::write(&temporary, &bytes).map_err(|error| error.to_string())?;

        // Sync the file
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);

        // Atomic rename
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;

        // Sync parent directory
        if let Some(parent) = destination.parent() {
            std::fs::File::open(parent)
                .and_then(|dir| dir.sync_all())
                .map_err(|error| error.to_string())?;
        }

        Ok(())
    }

    /// Convert from OutboxSnapshot to OutboxSnapshotRef
    pub fn outbox_snapshot_from(outbox: &OutboxSnapshot) -> OutboxSnapshotRef {
        OutboxSnapshotRef {
            path: outbox.path.clone(),
            size_bytes: outbox.size_bytes,
            checksum: outbox.checksum,
            materialized_lsn: outbox.materialized_lsn,
        }
    }
}

/// Manager for checkpoint manifests
pub struct CheckpointManifestManager {
    manifest_dir: PathBuf,
}

impl CheckpointManifestManager {
    /// Create a new manifest manager
    pub fn new(manifest_dir: impl AsRef<Path>) -> Self {
        Self {
            manifest_dir: manifest_dir.as_ref().to_path_buf(),
        }
    }

    /// Initialize the manifest directory
    pub fn init(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.manifest_dir).map_err(|error| error.to_string())?;

        // Sync directory
        std::fs::File::open(&self.manifest_dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|error| error.to_string())?;

        Ok(())
    }

    /// Get the path for a manifest file
    pub fn manifest_path(&self, checkpoint_id: u64) -> PathBuf {
        self.manifest_dir
            .join(format!("manifest_{:020}.postcard", checkpoint_id))
    }

    /// Publish a manifest atomically
    pub fn publish(&self, manifest: &CheckpointManifest) -> Result<(), String> {
        let path = self.manifest_path(manifest.checkpoint_id);
        manifest.save_atomic(&path)?;
        Ok(())
    }

    /// Load the latest published manifest
    pub fn load_latest(&self) -> Result<Option<CheckpointManifest>, String> {
        if !self.manifest_dir.exists() {
            return Ok(None);
        }

        let mut manifests: Vec<(u64, PathBuf)> = std::fs::read_dir(&self.manifest_dir)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                if !path.is_file() {
                    return None;
                }
                let name = path.file_name()?.to_str()?;
                let id = name
                    .strip_prefix("manifest_")?
                    .strip_suffix(".postcard")?
                    .parse::<u64>()
                    .ok()?;
                Some((id, path))
            })
            .collect();

        manifests.sort_by_key(|(id, _)| std::cmp::Reverse(*id));

        for (_, path) in manifests {
            match CheckpointManifest::load(&path) {
                Ok(manifest) => return Ok(Some(manifest)),
                Err(error) => {
                    log::warn!(
                        "Skipping invalid checkpoint manifest {}: {}",
                        path.display(),
                        error
                    );
                }
            }
        }
        Ok(None)
    }

    /// Load a specific manifest by checkpoint ID
    pub fn load(&self, checkpoint_id: u64) -> Result<Option<CheckpointManifest>, String> {
        let path = self.manifest_path(checkpoint_id);
        if !path.exists() {
            return Ok(None);
        }
        CheckpointManifest::load(path).map(Some)
    }

    /// List all published manifests
    pub fn list_manifests(&self) -> Result<Vec<u64>, String> {
        if !self.manifest_dir.exists() {
            return Ok(Vec::new());
        }

        let mut ids: Vec<u64> = std::fs::read_dir(&self.manifest_dir)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?;
                name.strip_prefix("manifest_")?
                    .strip_suffix(".postcard")?
                    .parse::<u64>()
                    .ok()
            })
            .collect();

        ids.sort();
        Ok(ids)
    }

    /// Get the safe LSN from the latest manifest
    pub fn latest_safe_lsn(&self) -> Result<CommitLsn, String> {
        self.load_latest()?
            .map(|m| m.safe_lsn)
            .ok_or_else(|| "No published manifest found".to_string())
    }

    /// Remove manifests older than the given checkpoint ID
    pub fn cleanup_old(&self, keep_newer_than: u64) -> Result<usize, String> {
        let mut removed = 0;
        for id in self.list_manifests()? {
            if id < keep_newer_than {
                let path = self.manifest_path(id);
                std::fs::remove_file(path).map_err(|error| error.to_string())?;
                removed += 1;
            }
        }

        // Sync directory
        if removed > 0 {
            std::fs::File::open(&self.manifest_dir)
                .and_then(|dir| dir.sync_all())
                .map_err(|error| error.to_string())?;
        }

        Ok(removed)
    }
}

fn verify_file_reference(
    path: &Path,
    expected_size: u64,
    expected_checksum: u32,
    kind: &str,
) -> Result<(), String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Failed to read {kind} {}: {error}", path.display()))?;
    if bytes.len() as u64 != expected_size {
        return Err(format!("{kind} size mismatch: {}", path.display()));
    }
    if crc32fast::hash(&bytes) != expected_checksum {
        return Err(format!("{kind} checksum mismatch: {}", path.display()));
    }
    Ok(())
}

fn collect_storage_files(root: &Path) -> Result<Vec<StorageFileRef>, String> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<StorageFileRef>) -> Result<(), String> {
        for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                visit(root, &path, files)?;
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            let relative = path
                .strip_prefix(root)
                .map_err(|error| error.to_string())?
                .to_path_buf();
            files.push(StorageFileRef {
                path: relative,
                size_bytes: bytes.len() as u64,
                checksum: crc32fast::hash(&bytes),
            });
        }
        Ok(())
    }

    if !root.is_dir() {
        return Err(format!(
            "Storage snapshot directory does not exist: {}",
            root.display()
        ));
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn storage_files_checksum(files: &[StorageFileRef]) -> Result<u32, String> {
    let bytes = postcard::to_allocvec(files).map_err(|error| error.to_string())?;
    Ok(crc32fast::hash(&bytes))
}

fn verify_storage_reference(reference: &StorageSnapshotRef) -> Result<(), String> {
    let actual = collect_storage_files(&reference.path)?;
    if actual != reference.files {
        return Err(format!(
            "Storage snapshot file list mismatch: {}",
            reference.path.display()
        ));
    }
    let size_bytes: u64 = actual.iter().map(|file| file.size_bytes).sum();
    if size_bytes != reference.size_bytes {
        return Err(format!(
            "Storage snapshot size mismatch: {}",
            reference.path.display()
        ));
    }
    let checksum = storage_files_checksum(&actual)?;
    if checksum != reference.checksum {
        return Err(format!(
            "Storage snapshot checksum mismatch: {}",
            reference.path.display()
        ));
    }

    for file in &actual {
        if file.path.is_absolute()
            || file
                .path
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(format!(
                "Invalid storage snapshot file path: {}",
                file.path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_storage_snapshot_ref(temp_dir: &Path) -> StorageSnapshotRef {
        let snapshot_path = temp_dir.join("storage_snapshot");
        std::fs::create_dir_all(&snapshot_path).unwrap();
        std::fs::write(snapshot_path.join("checkpoint.meta"), b"checkpoint").unwrap();
        CheckpointManifest::storage_snapshot_from_directory(&snapshot_path, 1, 100, 50).unwrap()
    }

    fn create_test_outbox_snapshot_ref(temp_dir: &Path) -> OutboxSnapshotRef {
        let snapshot_path = temp_dir.join("outbox_snapshot.sqlite");
        std::fs::write(&snapshot_path, b"test").unwrap();
        OutboxSnapshotRef {
            path: snapshot_path,
            size_bytes: 4,
            checksum: crc32fast::hash(b"test"),
            materialized_lsn: CommitLsn::new(100),
        }
    }

    #[test]
    fn test_manifest_creation_and_validation() {
        let temp_dir = TempDir::new().unwrap();
        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());
        let outbox_ref = create_test_outbox_snapshot_ref(temp_dir.path());

        let manifest = CheckpointManifest::new(
            1,
            CommitLsn::new(100),
            storage_ref,
            Some(outbox_ref),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");

        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.safe_lsn, CommitLsn::new(100));
        assert_eq!(manifest.storage_lsn, CommitLsn::new(100));
        assert_eq!(manifest.outbox_lsn, CommitLsn::new(100));
    }

    #[test]
    fn test_safe_lsn_is_minimum() {
        let temp_dir = TempDir::new().unwrap();
        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());

        // Storage LSN is lower. The outbox snapshot must not be ahead of the
        // storage boundary in a combined manifest.
        let manifest1 = CheckpointManifest::new(
            1,
            CommitLsn::new(50),
            storage_ref.clone(),
            Some(OutboxSnapshotRef {
                path: temp_dir.path().join("outbox.sqlite"),
                size_bytes: 4,
                checksum: 0,
                materialized_lsn: CommitLsn::new(50),
            }),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");
        assert_eq!(manifest1.safe_lsn, CommitLsn::new(50));

        // Outbox LSN is lower
        let manifest2 = CheckpointManifest::new(
            2,
            CommitLsn::new(100),
            storage_ref,
            Some(OutboxSnapshotRef {
                path: temp_dir.path().join("outbox2.sqlite"),
                size_bytes: 4,
                checksum: 0,
                materialized_lsn: CommitLsn::new(20),
            }),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");
        assert_eq!(manifest2.safe_lsn, CommitLsn::new(20));
    }

    #[test]
    fn enabled_outbox_without_snapshot_keeps_wal_reclaim_boundary_at_zero() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = CheckpointManifest::new_with_outbox(
            1,
            CommitLsn::new(100),
            create_test_storage_snapshot_ref(temp_dir.path()),
            None,
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");

        assert_eq!(manifest.safe_lsn, CommitLsn::ZERO);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_manifest_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());
        let outbox_ref = create_test_outbox_snapshot_ref(temp_dir.path());

        let manifest = CheckpointManifest::new(
            1,
            CommitLsn::new(100),
            storage_ref,
            Some(outbox_ref),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");

        let manifest_path = temp_dir.path().join("test_manifest.postcard");
        manifest.save_atomic(&manifest_path).unwrap();

        let loaded = CheckpointManifest::load(&manifest_path).unwrap();
        assert_eq!(loaded.checkpoint_id, manifest.checkpoint_id);
        assert_eq!(loaded.safe_lsn, manifest.safe_lsn);
    }

    fn create_storage_snapshot_ref(temp_dir: &Path, checkpoint_seq: u64) -> StorageSnapshotRef {
        let snapshot_path = temp_dir.join(format!("storage_snapshot_{}", checkpoint_seq));
        std::fs::create_dir_all(&snapshot_path).unwrap();
        std::fs::write(snapshot_path.join("checkpoint.meta"), b"checkpoint").unwrap();
        CheckpointManifest::storage_snapshot_from_directory(&snapshot_path, checkpoint_seq, 100, 50)
            .unwrap()
    }

    #[test]
    fn test_manifest_manager() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CheckpointManifestManager::new(temp_dir.path());
        manager.init().unwrap();

        let storage_ref1 = create_storage_snapshot_ref(temp_dir.path(), 1);
        let storage_ref2 = create_storage_snapshot_ref(temp_dir.path(), 2);
        let outbox_ref = create_test_outbox_snapshot_ref(temp_dir.path());

        let manifest1 = CheckpointManifest::new(
            1,
            CommitLsn::new(100),
            storage_ref1,
            Some(outbox_ref.clone()),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");
        let manifest2 = CheckpointManifest::new(
            2,
            CommitLsn::new(100),
            storage_ref2,
            Some(outbox_ref),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");

        manager.publish(&manifest1).unwrap();
        manager.publish(&manifest2).unwrap();

        let latest = manager.load_latest().unwrap().unwrap();
        assert_eq!(latest.checkpoint_id, 2);
        assert_eq!(latest.safe_lsn, CommitLsn::new(100));

        let safe_lsn = manager.latest_safe_lsn().unwrap();
        assert_eq!(safe_lsn, CommitLsn::new(100));

        let manifests = manager.list_manifests().unwrap();
        assert_eq!(manifests, vec![1, 2]);
    }

    #[test]
    fn test_checksum_verification() {
        let temp_dir = TempDir::new().unwrap();
        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());

        let mut manifest =
            CheckpointManifest::new(1, CommitLsn::new(100), storage_ref, None, Vec::new())
                .expect("checkpoint manifest should be created");

        assert!(manifest.verify_checksum().expect("checksum should compute"));

        // Corrupt the manifest
        manifest.checkpoint_id = 999;
        assert!(!manifest.verify_checksum().expect("checksum should compute"));
    }
}

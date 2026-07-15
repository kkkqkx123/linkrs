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

use graphdb_core::core::types::CommitLsn;
use serde::{Deserialize, Serialize};

use crate::sync::sqlite_outbox::OutboxSnapshot;

/// Combined checkpoint manifest that atomically references all snapshot components.
///
/// This is the core data structure for Phase 3. It ensures that:
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
    /// Common safe LSN - the minimum of storage_lsn and outbox_lsn
    /// This is the LSN up to which WAL can be safely truncated
    pub safe_lsn: CommitLsn,
    /// Reference to the storage snapshot
    pub storage_snapshot: StorageSnapshotRef,
    /// Reference to the outbox snapshot (if enabled)
    pub outbox_snapshot: Option<OutboxSnapshotRef>,
    /// References to native index manifests
    pub index_manifests: Vec<IndexManifestRef>,
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
    /// Checkpoint sequence number
    pub checkpoint_seq: u64,
    /// Number of vertices
    pub vertex_count: u64,
    /// Number of edges
    pub edge_count: u64,
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
    /// Index identifier
    pub index_id: u64,
    /// Index generation
    pub generation: u64,
    /// Path to the manifest file
    pub path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// CRC32 checksum
    pub checksum: u32,
}

impl CheckpointManifest {
    /// Current manifest format version
    pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// Create a new checkpoint manifest from component snapshots.
    ///
    /// This computes the common safe LSN as the minimum of storage and outbox LSNs.
    /// When there is no outbox snapshot, outbox_lsn is set to ZERO and safe_lsn
    /// equals storage_lsn.
    pub fn new(
        checkpoint_id: u64,
        storage_lsn: CommitLsn,
        storage_snapshot: StorageSnapshotRef,
        outbox_snapshot: Option<OutboxSnapshotRef>,
        index_manifests: Vec<IndexManifestRef>,
    ) -> Self {
        let outbox_lsn = outbox_snapshot
            .as_ref()
            .map(|s| s.materialized_lsn)
            .unwrap_or(CommitLsn::ZERO);

        // Safe LSN is the minimum of storage and outbox LSNs.
        // When there is no outbox snapshot, safe_lsn = storage_lsn.
        let safe_lsn = if outbox_snapshot.is_some() && outbox_lsn < storage_lsn {
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
            safe_lsn,
            storage_snapshot,
            outbox_snapshot,
            index_manifests,
            manifest_checksum: 0,
        };

        manifest.manifest_checksum = manifest.compute_checksum();
        manifest
    }

    /// Compute checksum of the manifest (excluding the checksum field itself)
    fn compute_checksum(&self) -> u32 {
        let mut clone = self.clone();
        clone.manifest_checksum = 0;
        let bytes = postcard::to_allocvec(&clone).unwrap_or_default();
        crc32fast::hash(&bytes)
    }

    /// Verify the manifest checksum
    pub fn verify_checksum(&self) -> bool {
        let expected = self.manifest_checksum;
        let mut clone = self.clone();
        clone.manifest_checksum = 0;
        let bytes = postcard::to_allocvec(&clone).unwrap_or_default();
        let computed = crc32fast::hash(&bytes);
        computed == expected
    }

    /// Validate the manifest structure and references
    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != Self::CURRENT_FORMAT_VERSION {
            return Err(format!(
                "Unsupported manifest format version: {}",
                self.format_version
            ));
        }

        if !self.verify_checksum() {
            return Err("Manifest checksum verification failed".to_string());
        }

        // Validate that safe_lsn is correctly computed
        let outbox_lsn = self
            .outbox_snapshot
            .as_ref()
            .map(|s| s.materialized_lsn)
            .unwrap_or(CommitLsn::ZERO);

        let expected_safe = if self.outbox_snapshot.is_some() && outbox_lsn < self.storage_lsn {
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

        // Validate that outbox LSN is consistent with outbox snapshot presence
        if self.outbox_snapshot.is_none() && self.outbox_lsn != CommitLsn::ZERO {
            return Err(
                "Outbox LSN is non-zero but outbox snapshot reference is missing".to_string(),
            );
        }

        Ok(())
    }

    /// Load a manifest from a file
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes = std::fs::read(path.as_ref()).map_err(|error| error.to_string())?;
        let manifest: Self =
            postcard::from_bytes(&bytes).map_err(|error| error.to_string())?;
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

        manifests
            .first()
            .map(|(_, path)| CheckpointManifest::load(path))
            .transpose()
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_storage_snapshot_ref(temp_dir: &Path) -> StorageSnapshotRef {
        let snapshot_path = temp_dir.join("storage_snapshot");
        std::fs::create_dir_all(&snapshot_path).unwrap();
        StorageSnapshotRef {
            path: snapshot_path,
            size_bytes: 1024,
            checksum: 0x12345678,
            checkpoint_seq: 1,
            vertex_count: 100,
            edge_count: 50,
        }
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
        );

        assert!(manifest.validate().is_ok());
        assert_eq!(manifest.safe_lsn, CommitLsn::new(100));
        assert_eq!(manifest.storage_lsn, CommitLsn::new(100));
        assert_eq!(manifest.outbox_lsn, CommitLsn::new(100));
    }

    #[test]
    fn test_safe_lsn_is_minimum() {
        let temp_dir = TempDir::new().unwrap();
        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());

        // Storage LSN is lower
        let manifest1 = CheckpointManifest::new(
            1,
            CommitLsn::new(50),
            storage_ref.clone(),
            Some(OutboxSnapshotRef {
                path: temp_dir.path().join("outbox.sqlite"),
                size_bytes: 4,
                checksum: 0,
                materialized_lsn: CommitLsn::new(100),
            }),
            Vec::new(),
        );
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
                materialized_lsn: CommitLsn::new(50),
            }),
            Vec::new(),
        );
        assert_eq!(manifest2.safe_lsn, CommitLsn::new(50));
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
        );

        let manifest_path = temp_dir.path().join("test_manifest.postcard");
        manifest.save_atomic(&manifest_path).unwrap();

        let loaded = CheckpointManifest::load(&manifest_path).unwrap();
        assert_eq!(loaded.checkpoint_id, manifest.checkpoint_id);
        assert_eq!(loaded.safe_lsn, manifest.safe_lsn);
    }

    #[test]
    fn test_manifest_manager() {
        let temp_dir = TempDir::new().unwrap();
        let manager = CheckpointManifestManager::new(temp_dir.path());
        manager.init().unwrap();

        let storage_ref = create_test_storage_snapshot_ref(temp_dir.path());
        let outbox_ref = create_test_outbox_snapshot_ref(temp_dir.path());

        let manifest1 = CheckpointManifest::new(
            1,
            CommitLsn::new(50),
            storage_ref.clone(),
            Some(outbox_ref.clone()),
            Vec::new(),
        );
        let manifest2 = CheckpointManifest::new(
            2,
            CommitLsn::new(100),
            storage_ref,
            Some(outbox_ref),
            Vec::new(),
        );

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

        let mut manifest = CheckpointManifest::new(
            1,
            CommitLsn::new(100),
            storage_ref,
            None,
            Vec::new(),
        );

        assert!(manifest.verify_checksum());

        // Corrupt the manifest
        manifest.checkpoint_id = 999;
        assert!(!manifest.verify_checksum());
    }
}

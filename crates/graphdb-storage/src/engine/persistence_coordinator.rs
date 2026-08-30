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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use parking_lot::RwLock;

use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;

use crate::engine::snapshot_manager::SnapshotManager;
use crate::engine::WalManager;
use crate::index::shard_runtime::IndexBarrierRegistry;
use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::wal::{CheckpointManager, Lsn, WalConfig};

#[path = "persistence/checkpoint.rs"]
pub mod checkpoint;
#[path = "persistence/config.rs"]
pub mod config;
#[path = "persistence/diagnostics.rs"]
pub mod diagnostics;

pub use checkpoint::{CheckpointData, CheckpointInfo, CheckpointStats, CHECKPOINT_FORMAT_VERSION};
pub use config::PersistenceConfig;
pub use diagnostics::{CatalogLockDiagnostic, PersistenceDiagnostics, SnapshotStats};

/// Type alias for the outbox frontier provider callback.
type OutboxFrontierProvider =
    Arc<dyn Fn() -> StorageResult<Option<graphdb_core::types::CommitLsn>> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceState {
    Idle,
    Checkpointing,
    Snapshotting,
}

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

pub(crate) struct PersistenceStateGuard<'a> {
    pub(crate) state: &'a RwLock<PersistenceState>,
}

impl Drop for PersistenceStateGuard<'_> {
    fn drop(&mut self) {
        *self.state.write() = PersistenceState::Idle;
    }
}

pub struct PersistenceCoordinator {
    pub(crate) config: PersistenceConfig,
    pub(crate) wal_manager: Option<Arc<RwLock<WalManager>>>,
    pub(crate) checkpoint_manager: RwLock<CheckpointManager>,
    pub(crate) snapshot_manager: Option<Arc<SnapshotManager>>,
    pub(crate) manifest_manager: CheckpointManifestManager,
    pub(crate) last_checkpoint_time: RwLock<Instant>,
    pub(crate) last_flush_time: RwLock<Instant>,
    pub(crate) last_checkpoint_lsn: RwLock<Lsn>,
    pub(crate) last_flush_lsn: RwLock<Lsn>,
    pub(crate) last_snapshot_time: RwLock<Option<SystemTime>>,
    pub(crate) last_checkpoint_error: RwLock<Option<String>>,
    pub(crate) last_snapshot_error: RwLock<Option<String>>,
    pub(crate) state: RwLock<PersistenceState>,
    pub(crate) fault_points: Arc<RwLock<HashSet<PersistenceFaultPoint>>>,
    pub(crate) outbox_frontier_provider: RwLock<Option<OutboxFrontierProvider>>,
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
            graphdb_core::StorageError::db_error(format!(
                "Failed to init checkpoint manager: {}",
                e
            ))
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
            outbox_frontier_provider: RwLock::new(None),
        })
    }

    /// Enable a deterministic failure at a persistence boundary.
    #[cfg(test)]
    pub fn inject_failure(&self, point: PersistenceFaultPoint) {
        self.fault_points.write().insert(point);
    }

    pub(crate) fn fail_if_injected(&self, point: PersistenceFaultPoint) -> StorageResult<()> {
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

    pub(crate) fn set_index_barrier_registry(&mut self, registry: IndexBarrierRegistry) {
        if let Some(wal) = &self.wal_manager {
            wal.write().set_index_barrier_registry(registry);
        }
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

    pub(crate) fn enter_state(
        &self,
        state: PersistenceState,
    ) -> StorageResult<PersistenceStateGuard<'_>> {
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

    pub fn set_outbox_materialized_lsn_provider(&self, provider: OutboxFrontierProvider) {
        *self.outbox_frontier_provider.write() = Some(provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;
    use tempfile::TempDir;

    use crate::engine::config::PropertyGraphConfig;
    use graphdb_transaction::wal::SyncPolicy;

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
    fn checkpoint_stats_report_the_manifest_safe_lsn() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = PersistenceConfig {
            enable_snapshots: false,
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
            ..PersistenceConfig::for_work_dir(temp_dir.path())
        };
        let coordinator = PersistenceCoordinator::new(config).expect("coordinator");

        let wal = coordinator.wal_manager().expect("WAL should be enabled");
        wal.read()
            .append_redo(graphdb_core::wal::types::WalOpType::Compact, 1, &())
            .expect("WAL entry should append");
        wal.read().sync().expect("WAL should be durable");
        let checkpoint_lsn = wal.read().durable_lsn();
        assert!(checkpoint_lsn > Lsn::ZERO);

        let snapshot_dir = temp_dir.path().join("outbox_snapshots");
        std::fs::create_dir_all(&snapshot_dir).expect("snapshot directory should exist");
        let snapshot_path = snapshot_dir.join("outbox_snapshot_0.sqlite");
        let snapshot_bytes = b"valid checksum fixture";
        std::fs::write(&snapshot_path, snapshot_bytes).expect("snapshot should be written");
        std::fs::write(
            snapshot_path.with_extension("checksum"),
            crc32fast::hash(snapshot_bytes).to_string(),
        )
        .expect("snapshot checksum should be written");

        let stats = coordinator
            .create_checkpoint(
                |temporary_dir, _| {
                    std::fs::write(temporary_dir.join("table.data"), b"data")?;
                    Ok(CheckpointData {
                        vertex_count: 1,
                        edge_count: 0,
                        data_size: 4,
                    })
                },
                1,
            )
            .expect("checkpoint should succeed");

        assert_eq!(stats.wal_truncated, 0);
        assert!(stats.wal_truncated < checkpoint_lsn.into());
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
                            vertex_count: sequence,
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
                            vertex_count: sequence,
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

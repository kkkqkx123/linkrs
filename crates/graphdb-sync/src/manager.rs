//! Sync Manager
//!
//! Unified synchronization manager using SyncCoordinator.
//!
//! The implementation is split by responsibility across submodules:
//! `types` (shared types), `lifecycle` (construction and runtime),
//! `intents` (transactional staging), `ingest` (mutation entry points),
//! `delivery` (outbox claim-apply-ack), `outbox_admin` (snapshots, dead
//! letters, retention) and `accessors` (read-only views and facades).
//! This module keeps the shared state, the sync-execution primitive and
//! the unit tests.

mod accessors;
mod delivery;
mod ingest;
mod intents;
mod lifecycle;
mod outbox_admin;
mod types;

#[cfg(feature = "vector")]
pub(crate) use self::intents::format_vector_point_id;
#[cfg(test)]
pub(crate) use self::intents::payload_to_intent;
pub(crate) use self::intents::stable_hash;
pub use self::types::{
    EdgeProps, EdgeRef, IndexCreateRequest, OutboxBackpressureConfig, OutboxConsumerConfig,
    SyncError,
};

#[cfg(feature = "fulltext")]
use crate::coordinator::SyncCoordinator;
use crate::sqlite_outbox::SqliteOutbox;
#[cfg(feature = "vector")]
use crate::vector_sync::VectorSyncCoordinator;
use dashmap::DashMap;
use graphdb_core::types::TransactionId;
use graphdb_metrics::StatsManager;
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(feature = "vector")]
pub use vector_search::{CollectionConfig, SearchResult};

type JoinHandleGuard = Mutex<Option<tokio::task::JoinHandle<()>>>;

pub struct SyncManager {
    #[cfg(feature = "fulltext")]
    sync_coordinator: Option<Arc<SyncCoordinator>>,
    #[cfg(feature = "vector")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pending_intents: DashMap<TransactionId, Vec<graphdb_core::wal::OutboxIntent>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    dead_letter_queue: Option<Arc<crate::DeadLetterQueue>>,
    sqlite_outbox: Option<Arc<SqliteOutbox>>,
    #[cfg(feature = "vector")]
    vector_receiver: Option<Arc<crate::VectorReceiver>>,
    outbox_consumer: Arc<OutboxConsumerConfig>,
    #[cfg(feature = "vector")]
    backend_policy: Option<Arc<crate::backend::BackendDeliveryPolicy>>,
    backpressure: Arc<OutboxBackpressureConfig>,
    /// When an `Auth` error is seen, delivery is paused until this timestamp
    /// (millis since epoch). Atomic for lock-free reads in the hot path.
    auth_paused_until_ms: Arc<std::sync::atomic::AtomicU64>,
    stats_manager: Option<Arc<StatsManager>>,
    handle: JoinHandleGuard,
    /// Per-index rebuild admission locks. A rebuild holds its key across
    /// generation registration, backfill, catch-up, and publish, so two
    /// concurrent rebuilds of the same index serialize: the loser gets
    /// `SyncError::RebuildBusy` instead of interleaving generations.
    rebuild_locks: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            #[cfg(feature = "fulltext")]
            sync_coordinator: self.sync_coordinator.clone(),
            #[cfg(feature = "vector")]
            vector_coordinator: self.vector_coordinator.clone(),
            pending_intents: self.pending_intents.clone(),
            running: self.running.clone(),
            dead_letter_queue: self.dead_letter_queue.clone(),
            sqlite_outbox: self.sqlite_outbox.clone(),
            #[cfg(feature = "vector")]
            vector_receiver: self.vector_receiver.clone(),
            outbox_consumer: self.outbox_consumer.clone(),
            #[cfg(feature = "vector")]
            backend_policy: self.backend_policy.clone(),
            backpressure: self.backpressure.clone(),
            auth_paused_until_ms: self.auth_paused_until_ms.clone(),
            stats_manager: self.stats_manager.clone(),
            handle: Mutex::new(None),
            rebuild_locks: self.rebuild_locks.clone(),
        }
    }
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SyncManager");
        #[cfg(feature = "fulltext")]
        d.field("sync_coordinator", &self.sync_coordinator);
        #[cfg(feature = "vector")]
        d.field("vector_coordinator", &self.vector_coordinator);
        d.field("running", &self.running);
        d.finish_non_exhaustive()
    }
}

impl SyncManager {
    fn execute_sync<F, Fut, T>(&self, f: F) -> Result<T, SyncError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, SyncError>> + Send,
        T: Send,
    {
        crate::runtime::block_on_ambient(f())
            .map_err(|error| SyncError::Internal(error.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint_manifest::CheckpointManifestManager;
    use crate::outbox::OutboxPayload;
    use crate::sqlite_outbox::{OutboxSnapshot, SqliteOutbox};
    use crate::types::ChangeType;
    use crate::CheckpointManifest;
    use graphdb_core::types::{CommitLsn, TransactionId};
    use graphdb_core::Value;
    use std::path::Path;
    use tempfile::TempDir;

    async fn create_test_snapshots(root: &Path) -> Vec<OutboxSnapshot> {
        let source = root.join("source.sqlite");
        let outbox = SqliteOutbox::open(&source)
            .await
            .expect("source outbox should open");
        let snapshot_dir = root.join("outbox_snapshots");
        let mut snapshots = Vec::new();
        for lsn in [100, 200] {
            outbox
                .materialize_commit(CommitLsn::new(lsn), &[], &[])
                .await
                .expect("source outbox should materialize");
            snapshots.push(
                outbox
                    .create_snapshot(snapshot_dir.join(format!("outbox_snapshot_{lsn}.sqlite")))
                    .await
                    .expect("outbox snapshot should be created"),
            );
        }
        snapshots
    }

    fn publish_test_manifest(root: &Path, snapshot: &OutboxSnapshot) {
        let storage_path = root.join("checkpoint/checkpoint_1");
        std::fs::create_dir_all(&storage_path).expect("storage checkpoint should exist");
        std::fs::write(storage_path.join("checkpoint.meta"), b"checkpoint")
            .expect("storage checkpoint metadata should exist");
        let manifest = CheckpointManifest::new(
            1,
            snapshot.materialized_lsn,
            CheckpointManifest::storage_snapshot_from_directory(&storage_path, 1, 0, 0)
                .expect("storage snapshot reference should be created"),
            Some(CheckpointManifest::outbox_snapshot_from(snapshot)),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");
        CheckpointManifestManager::new(root.join("checkpoint/manifests"))
            .publish(&manifest)
            .expect("checkpoint manifest should publish");
    }

    #[test]
    fn pending_intents_are_available_before_commit() {
        let manager = SyncManager::new_without_fulltext();
        let txn_id = TransactionId::new(77);
        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[],
                ChangeType::Insert,
            )
            .expect("event should stage");
        let intents = manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available");
        assert!(intents.is_empty());
        assert_eq!(manager.outbox_stats().pending, 0);
    }

    #[test]
    fn pending_intents_are_cleared_on_rollback() {
        let manager = SyncManager::new_without_fulltext();
        let txn_id = TransactionId::new(88);
        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[],
                ChangeType::Insert,
            )
            .expect("event should stage");
        assert!(manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available")
            .is_empty());
        manager.clear_transaction_intents(txn_id);
        assert_eq!(manager.outbox_stats().pending, 0);
    }

    #[test]
    fn edge_delete_intent_preserves_parallel_edge_ranking() {
        let payload = OutboxPayload::EdgeDelete {
            space_id: 1,
            src: Value::string("src"),
            dst: Value::string("dst"),
            edge_type: "KNOWS".to_string(),
            ranking: 7,
        };
        let intent = payload_to_intent(TransactionId::new(101), 0, "fulltext", &payload)
            .expect("edge delete intent should be serializable");

        match intent.mutation.entity_ref {
            graphdb_core::wal::EntityRef::Edge { ranking, .. } => assert_eq!(ranking, 7),
            other => panic!("expected edge entity reference, got {other:?}"),
        }
    }

    #[cfg(feature = "fulltext")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fulltext_changes_use_only_the_fulltext_target() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let config = graphdb_fulltext::FulltextConfig {
            index_path: directory.path().to_path_buf(),
            ..Default::default()
        };
        let fulltext_manager = Arc::new(
            graphdb_fulltext::FulltextIndexManager::new(config)
                .expect("fulltext manager should be created"),
        );
        fulltext_manager
            .create_index(1, "Node", "text", None)
            .await
            .expect("fulltext index should be created");
        let coordinator = Arc::new(crate::SyncCoordinator::new(
            fulltext_manager,
            crate::BatchConfig::default(),
        ));
        let manager = SyncManager::new(coordinator);
        let outbox_path = directory.path().join("outbox/outbox.sqlite");
        let mut manager = manager;
        manager
            .configure_outbox(outbox_path)
            .expect("test outbox should be configured");
        let txn_id = TransactionId::new(99);

        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[("text".to_string(), Value::string("hello"))],
                ChangeType::Insert,
            )
            .expect("change should stage");

        let intents = manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available");
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].mutation.target.as_str(), "fulltext");
    }

    #[cfg(feature = "fulltext")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fulltext_outbox_claim_apply_and_restart_receipt_are_end_to_end() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let config = graphdb_fulltext::FulltextConfig {
            index_path: directory.path().join("indexes"),
            ..Default::default()
        };
        let fulltext_manager = Arc::new(
            graphdb_fulltext::FulltextIndexManager::new(config)
                .expect("fulltext manager should be created"),
        );
        fulltext_manager
            .create_index(1, "Node", "text", None)
            .await
            .expect("receiver index should be created");
        let coordinator = Arc::new(crate::SyncCoordinator::new(
            fulltext_manager.clone(),
            crate::BatchConfig::default(),
        ));
        let mut manager = SyncManager::new(coordinator);
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should be configured");
        let transaction_id = TransactionId::new(1001);
        manager
            .on_vertex_change_with_txn(
                transaction_id,
                1,
                "Node",
                &Value::string("node-1"),
                &[("text".to_string(), Value::string("durable graph event"))],
                ChangeType::Insert,
            )
            .expect("change should stage");
        let intents = manager
            .pending_transaction_intents(transaction_id)
            .expect("staged intents should load");
        manager
            .materialize_committed_transaction(transaction_id, CommitLsn::new(10), &intents)
            .expect("commit should materialize");
        manager.clear_transaction_intents(transaction_id);
        assert_eq!(manager.retry_outbox_sync().expect("claim should apply"), 1);
        let results = fulltext_manager
            .search(1, "Node", "text", "durable", 10)
            .await
            .expect("search should see the applied event");
        assert_eq!(results.len(), 1);

        let recovered = crate::receiver::FulltextReceiver::new(
            fulltext_manager
                .get_engine(1, "Node", "text")
                .expect("receiver engine should exist"),
        );
        let duplicate = recovered
            .check_late_arrival(CommitLsn::new(10), "1001:fulltext:1:text")
            .await;
        assert!(
            !duplicate.accepted,
            "field-specific receipt should reject a duplicate"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_prefers_manifest_snapshot_over_newer_directory_snapshot() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        publish_test_manifest(directory.path(), &snapshots[0]);

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should recover from the manifest snapshot");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(100))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_verifies_and_restores_corrupt_live_database() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        publish_test_manifest(directory.path(), &snapshots[0]);
        let live_path = directory.path().join("outbox/outbox.sqlite");
        std::fs::create_dir_all(live_path.parent().expect("live parent should exist"))
            .expect("live parent should be created");
        std::fs::write(&live_path, b"corrupt sqlite").expect("live database should be corrupt");

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(&live_path)
            .expect("corrupt outbox should be restored");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(100))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_falls_back_when_manifest_snapshot_checksum_is_invalid() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        std::fs::write(&snapshots[0].path, b"corrupt snapshot")
            .expect("manifest snapshot should be corrupt");
        publish_test_manifest(directory.path(), &snapshots[0]);

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should fall back to a valid snapshot");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(200))
        );
    }
}

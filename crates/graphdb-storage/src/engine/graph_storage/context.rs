use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::engine::background_freeze::BackgroundFreezeManager;
use crate::engine::config::PropertyGraphConfig;
use crate::engine::data_store::GraphDataStore;
use crate::engine::paths::StoragePaths;
use crate::index::{IndexDataManagerImpl, IndexGcConfig, IndexGcManager};
use crate::vertex::gc_manager::VertexGcManager;
use crate::StorageOperationContext;
use graphdb_core::types::Timestamp;
use graphdb_transaction::undo_log::UndoLogManager;
use graphdb_transaction::VersionManager;

pub mod autocommit;
pub mod evidence;
pub mod persistent;
pub(crate) mod txn_staging;

pub use autocommit::{AutoCommitBatchWindow, WriteGateStats};
pub(crate) use autocommit::{AutoCommitMutationRecorder, AutoCommitWriteLease};
pub(crate) use evidence::VertexIdDomainEvidence;
pub(crate) use persistent::GraphStoragePersistent;
pub(crate) use txn_staging::TxnStaging;

#[derive(Clone)]
pub(crate) struct GraphStorageLayout {
    work_dir: Option<PathBuf>,
    db_path: String,
}

impl GraphStorageLayout {
    fn new() -> Self {
        Self {
            work_dir: None,
            db_path: String::new(),
        }
    }

    fn new_with_path(path: PathBuf) -> Self {
        Self {
            work_dir: Some(path.clone()),
            db_path: path.to_string_lossy().to_string(),
        }
    }

    fn work_dir(&self) -> &Option<PathBuf> {
        &self.work_dir
    }

    fn storage_paths(&self) -> Option<StoragePaths> {
        self.work_dir.as_ref().cloned().map(StoragePaths::new)
    }

    fn spill_dir(&self) -> PathBuf {
        self.work_dir
            .as_ref()
            .map(|p| p.join("spill"))
            .unwrap_or_else(|| PathBuf::from("/tmp/linkrs_spill"))
    }

    fn db_path(&self) -> &str {
        &self.db_path
    }
}

#[derive(Clone)]
/// Deferred WAL operations for two-phase recovery.
/// Used to handle edge operations that depend on vertex existence.
struct DeferredWalOps {
    /// Deferred edge insertions (InsertEdgeRedo, Timestamp)
    edges: Arc<Mutex<Vec<(graphdb_core::wal::redo::InsertEdgeRedo, Timestamp)>>>,
    /// Deferred edge deletions (DeleteEdgeRedo, Timestamp)
    deletes: Arc<Mutex<Vec<(graphdb_core::wal::redo::DeleteEdgeRedo, Timestamp)>>>,
}

impl DeferredWalOps {
    fn new() -> Self {
        Self {
            edges: Arc::new(Mutex::new(Vec::new())),
            deletes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn push_edge(&self, edge: graphdb_core::wal::redo::InsertEdgeRedo, ts: Timestamp) {
        self.edges.lock().push((edge, ts));
    }

    fn push_delete(&self, delete: graphdb_core::wal::redo::DeleteEdgeRedo, ts: Timestamp) {
        self.deletes.lock().push((delete, ts));
    }

    fn drain_edges(&self) -> Vec<(graphdb_core::wal::redo::InsertEdgeRedo, Timestamp)> {
        self.edges.lock().drain(..).collect()
    }

    fn drain_deletes(&self) -> Vec<(graphdb_core::wal::redo::DeleteEdgeRedo, Timestamp)> {
        self.deletes.lock().drain(..).collect()
    }
}

#[derive(Clone)]
struct GraphStorageRuntime {
    index_gc_manager: Option<Arc<IndexGcManager>>,
    vertex_gc_manager: Option<Arc<VertexGcManager>>,
    background_freeze_manager: Option<Arc<BackgroundFreezeManager>>,
    deferred_wal_ops: DeferredWalOps,
    /// Shared rayon-backed pool for background maintenance work (freeze,
    /// vertex/index GC loops). See [`crate::thread_pool`].
    thread_pool: Arc<crate::thread_pool::StorageThreadPool>,
    background_freeze_running: Arc<AtomicBool>,
    /// Last automatic vertex compaction time, for cooldown checks
    last_auto_compact: Arc<Mutex<Option<std::time::Instant>>>,
    /// Last index-GC pass time, for throttling opportunistic GC.
    last_index_gc: Arc<Mutex<Option<std::time::Instant>>>,
}

struct WriteTimestampLease {
    version_manager: Arc<VersionManager>,
    timestamp: Timestamp,
    finalized: AtomicBool,
}

impl Drop for WriteTimestampLease {
    fn drop(&mut self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            self.version_manager.abort_write_timestamp(self.timestamp);
        }
    }
}

impl WriteTimestampLease {
    fn abort(&self) {
        if !self.finalized.swap(true, Ordering::SeqCst) {
            self.version_manager.abort_write_timestamp(self.timestamp);
        }
    }
}

impl GraphStorageRuntime {
    fn new() -> Self {
        Self {
            index_gc_manager: None,
            vertex_gc_manager: None,
            background_freeze_manager: None,
            deferred_wal_ops: DeferredWalOps::new(),
            thread_pool: Arc::new(crate::thread_pool::StorageThreadPool::new().unwrap_or_else(
                |e| {
                    log::error!("Failed to build storage thread pool: {}", e);
                    crate::thread_pool::StorageThreadPool::default()
                },
            )),
            background_freeze_running: Arc::new(AtomicBool::new(false)),
            last_auto_compact: Arc::new(Mutex::new(None)),
            last_index_gc: Arc::new(Mutex::new(None)),
        }
    }

    fn with_index_gc(
        &self,
        index_data_manager: &Arc<RwLock<IndexDataManagerImpl>>,
        version_manager: &Arc<VersionManager>,
        config: IndexGcConfig,
    ) -> Self {
        let index_data = index_data_manager.read().clone();
        let gc_manager = IndexGcManager::new(
            index_data,
            version_manager.clone(),
            config,
            self.thread_pool.clone(),
        );

        Self {
            index_gc_manager: Some(Arc::new(gc_manager)),
            vertex_gc_manager: self.vertex_gc_manager.clone(),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            thread_pool: self.thread_pool.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn with_vertex_gc(
        &self,
        data_store: &Arc<GraphDataStore>,
        version_manager: &Arc<VersionManager>,
        config: crate::vertex::VertexGcConfig,
    ) -> Self {
        let gc_manager = VertexGcManager::new(
            data_store.clone(),
            version_manager.clone(),
            config,
            self.thread_pool.clone(),
        );

        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            vertex_gc_manager: Some(Arc::new(gc_manager)),
            background_freeze_manager: self.background_freeze_manager.clone(),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            thread_pool: self.thread_pool.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn with_background_freeze(&self, manager: Arc<BackgroundFreezeManager>) -> Self {
        Self {
            index_gc_manager: self.index_gc_manager.clone(),
            vertex_gc_manager: self.vertex_gc_manager.clone(),
            background_freeze_manager: Some(manager),
            deferred_wal_ops: self.deferred_wal_ops.clone(),
            thread_pool: self.thread_pool.clone(),
            background_freeze_running: self.background_freeze_running.clone(),
            last_auto_compact: self.last_auto_compact.clone(),
            last_index_gc: self.last_index_gc.clone(),
        }
    }

    fn start_index_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.index_gc_manager
            .as_ref()
            .map(|gc: &Arc<IndexGcManager>| gc.start_background_gc())
    }

    fn stop_index_gc(&self) {
        if let Some(ref gc) = self.index_gc_manager {
            gc.stop();
        }
    }

    fn is_index_gc_running(&self) -> bool {
        self.index_gc_manager
            .as_ref()
            .map(|g: &Arc<IndexGcManager>| g.is_running())
            .unwrap_or(false)
    }

    fn maybe_run_index_gc(&self) {
        let Some(gc) = self.index_gc_manager.as_ref() else {
            return;
        };
        {
            let mut last = self.last_index_gc.lock();
            if last
                .as_ref()
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(2))
            {
                return;
            }
            *last = Some(std::time::Instant::now());
        }
        let stats = gc.run_gc_pass();
        if !stats.is_empty() {
            log::debug!(
                "Opportunistic index GC removed {} entries",
                stats.total_removed()
            );
        }
    }

    fn start_vertex_gc(&self) -> Option<crate::thread_pool::BackgroundTaskHandle> {
        self.vertex_gc_manager
            .as_ref()
            .map(|gc: &Arc<VertexGcManager>| gc.start_background_gc())
    }

    fn stop_vertex_gc(&self) {
        if let Some(ref gc) = self.vertex_gc_manager {
            gc.stop();
        }
    }

    fn is_vertex_gc_running(&self) -> bool {
        self.vertex_gc_manager
            .as_ref()
            .map(|g: &Arc<VertexGcManager>| g.is_running())
            .unwrap_or(false)
    }
}

#[derive(Clone)]
pub struct GraphStorageContext {
    persistent: GraphStoragePersistent,
    runtime: GraphStorageRuntime,
    operation_context: Option<Arc<StorageOperationContext>>,
    write_timestamp_lease: Option<Arc<WriteTimestampLease>>,
    /// Held while an auto-commit DML statement executes (see
    /// [`AutoCommitWriteGate`]); released on finalize or Drop.
    write_gate_lease: Option<Arc<AutoCommitWriteLease>>,
    /// Before-image undo log for the active auto-commit statement; applied on
    /// finalize(false) to roll back partial writes (see
    /// [`AutoCommitMutationRecorder`]).
    auto_commit_undo: Option<Arc<Mutex<UndoLogManager>>>,
    /// Write set of the active auto-commit statement, shared with its
    /// [`AutoCommitMutationRecorder`]; certified at commit against recently
    /// committed write sets.
    auto_commit_write_set: Option<Arc<parking_lot::Mutex<graphdb_transaction::types::WriteSet>>>,
    /// When `Some`, this context is bound inside an [`AutoCommitBatchWindow`]
    /// MVCC snapshots and the write-gate lease are owned by the window
    /// and shared across all statements of the batch. `finalize_operation`
    /// therefore skips per-statement snapshot unregistration and never
    /// releases the window's write gate.
    auto_commit_window: Option<Arc<AutoCommitBatchWindow>>,
    checkpoint_scheduler:
        Arc<Mutex<Option<crate::engine::persistence_coordinator::CheckpointScheduler>>>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Module organization: split into logical groups
// ──────────────────────────────────────────────────────────────────────────────

mod accessors;
mod cache_index;
pub(crate) mod conflict;
mod edge_ops;
mod freeze;
pub(crate) mod helpers;
mod init;
mod maintenance;
mod persistence;
mod query;
mod schema;
pub(crate) mod vertex_ops;

impl std::fmt::Debug for GraphStorageContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphStorageContext").finish()
    }
}

impl GraphStorageContext {
    pub fn new_with_config(config: PropertyGraphConfig) -> graphdb_core::StorageResult<Self> {
        let persistent = GraphStoragePersistent::new_with_config(config)?;
        if let Err(e) = persistent.spiller.cleanup_stale_files() {
            log::warn!("Failed to clean up stale spill files: {}", e);
        }
        crate::vertex::column::chunk_residency::cleanup_stale_spill_dirs();
        Ok(Self {
            persistent,
            runtime: GraphStorageRuntime::new(),
            operation_context: None,
            write_timestamp_lease: None,
            write_gate_lease: None,
            auto_commit_undo: None,
            auto_commit_write_set: None,
            auto_commit_window: None,
            checkpoint_scheduler: Arc::new(Mutex::new(None)),
        })
    }

    /// Cumulative auto-commit write-gate admission statistics. Used by
    /// benchmarks (B2) to attribute write-path time to gate contention.
    pub fn write_gate_stats(&self) -> WriteGateStats {
        self.persistent.auto_commit_write_gate.stats()
    }

    /// Current outbox pending depth (staged + durable) for backpressure
    /// observability. Returns `0` when no stats manager is configured.
    pub fn outbox_pending(&self) -> u64 {
        self.persistent
            .stats_manager
            .as_ref()
            .and_then(|m| m.get_value(graphdb_metrics::MetricType::OutboxPending))
            .unwrap_or(0)
    }

    /// Lazily register a vertex label snapshot if not already registered.
    /// Returns the snapshot handle if registration succeeded.
    ///
    /// Read-only statement contexts (see `with_read_operation_context`) pin
    /// their read timestamp the same way auto-commit writes pin their write
    /// timestamp, so a long-running read cannot be unterminated by GC while
    /// the statement still observes the snapshot.
    pub(crate) fn record_cache_eligible(&self, ts: Timestamp) -> bool {
        // The record cache holds one entry per key: the value current at
        // `cached_at_ts`, served to every reader at or past it. Only
        // snapshots at the committed frontier observe current values, so
        // only they may consult or populate the cache. Older snapshots
        // must use the version chain: caching a historical value would
        // poison later frontier readers with stale data.
        ts >= self.persistent.version_manager.read_timestamp()
    }

    /// Build the per-pass GC coordinator with checkpoint bounds attached.
    ///
    /// Fresh checkpoint values come from the persistence watermark cell;
    /// after a restart the cell starts empty and the coordinator self-heals
    /// with one lazy manifest load per capture (see `GcCoordinator`).
    pub(crate) fn gc_coordinator(&self) -> crate::engine::gc_coordinator::GcCoordinator {
        let mut gc = crate::engine::gc_coordinator::GcCoordinator::new(
            self.persistent.version_manager.clone(),
        );
        if let Some(persistence) = self.persistent.persistence.as_ref() {
            let guard = persistence.read();
            gc = gc.with_manifest_dir(guard.checkpoint_dir().join("manifests"));
            let (snapshot, reclaim_lsn) = guard.checkpoint_watermark();
            if let Some(snapshot) = snapshot {
                gc.refresh_checkpoint_watermark(snapshot, reclaim_lsn);
            }
        }
        gc
    }
}

impl Default for GraphStorageContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::thread;

    use graphdb_core::types::EdgeIdentifier;
    use graphdb_transaction::TransactionMutationRecorder;
    use graphdb_transaction::{MutationEntityKey, MutationResult, VertexId};

    use super::autocommit::AutoCommitWriteGate;

    #[test]
    fn test_auto_commit_write_gate_mutual_exclusion() {
        let gate = AutoCommitWriteGate::new();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let gate = gate.clone();
            let active = active.clone();
            let max_active = max_active.clone();
            handles.push(thread::spawn(move || {
                let _lease = gate.acquire();
                let cur = active.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                max_active.fetch_max(cur, AtomicOrdering::SeqCst);
                thread::sleep(std::time::Duration::from_millis(5));
                active.fetch_sub(1, AtomicOrdering::SeqCst);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(
            max_active.load(AtomicOrdering::SeqCst),
            1,
            "auto-commit write gate must never admit more than one writer"
        );
    }

    #[test]
    fn test_auto_commit_write_gate_release_is_idempotent() {
        let gate = AutoCommitWriteGate::new();
        let lease = gate.acquire();
        lease.release();
        // Releasing twice (release + Drop) must not panic or double-notify.
        drop(lease);
        // The gate must accept a new writer after release.
        let next = gate.acquire();
        drop(next);
    }

    #[test]
    fn test_auto_commit_write_gate_is_reentrant_per_thread() {
        // Regression: a statement-level auto-commit binding holds the gate
        // while a nested gated operation on the same thread (COPY FROM group
        // window) acquires it again. Non-reentrant admission self-deadlocked.
        let gate = AutoCommitWriteGate::new();
        let outer = gate.acquire();
        {
            // Same-thread re-acquisition must not block.
            let inner = gate.acquire();
            let innermost = gate.acquire();
            drop(inner);
            drop(innermost);
        }
        // Depth still 1 after the nested leases dropped: another thread must
        // stay excluded until `outer` releases.
        let spawned = thread::spawn({
            let gate = gate.clone();
            move || {
                let lease = gate.acquire();
                drop(lease);
                true
            }
        });
        thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !spawned.is_finished(),
            "gate admitted a second holder while leased"
        );
        drop(outer);
        assert!(
            spawned.join().unwrap(),
            "waiter must acquire after outer release"
        );
    }

    #[test]
    fn test_auto_commit_recorder_forwards_entity_keys_to_write_set() {
        let recorder = AutoCommitMutationRecorder {
            undo: Arc::new(Mutex::new(UndoLogManager::new())),
            write_set: Arc::new(Mutex::new(graphdb_transaction::types::WriteSet::new())),
        };

        let vertex_id = VertexId::try_from_int64(42).expect("test vertex id");
        let edge = EdgeIdentifier {
            src_label: 3,
            src_vid: VertexId::try_from_int64(1).expect("test vertex id"),
            dst_label: 4,
            dst_vid: VertexId::try_from_int64(2).expect("test vertex id"),
            edge_label: 5,
            rank: 0,
        };

        recorder
            .record_mutation(MutationResult {
                entity_keys: vec![
                    MutationEntityKey::Vertex(vertex_id),
                    MutationEntityKey::Edge(edge),
                ],
                ..MutationResult::default()
            })
            .unwrap();

        let write_set = recorder.write_set.lock();
        assert!(write_set.vertices.contains(&vertex_id));
        assert!(write_set.edges.contains(&edge));
    }
}

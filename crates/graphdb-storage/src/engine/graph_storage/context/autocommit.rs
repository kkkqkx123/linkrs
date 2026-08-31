use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use graphdb_core::types::EdgeIdentifier;
use graphdb_core::types::LabelId;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult};
use graphdb_transaction::undo_log::UndoLogManager;
use graphdb_transaction::{
    MutationEntityKey, MutationResult, TransactionError, UndoLogEntry, VertexId,
};

use crate::engine::data_store::EdgeTableKey;
use crate::mvcc::SnapshotHandle;
use crate::StorageOperationContext;

/// Cumulative gate admission statistics (acquisitions and total wait time).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteGateStats {
    /// Number of gate acquisitions (statements serialized).
    pub acquisitions: u64,
    /// Total time spent waiting for admission, in nanoseconds.
    pub wait_nanos: u64,
}

/// Serializes auto-commit DML statements.
pub(crate) struct AutoCommitWriteGate {
    /// Gate state, guarded by `mutex`: the holder thread and its lease depth.
    ///
    /// The gate is **re-entrant per thread**: a statement-level auto-commit
    /// binding holds the gate while the statement executes, and nested gated
    /// operations on the same thread (e.g. `COPY FROM` opening its group
    /// window inside that statement) must not self-deadlock. Nested leases
    /// only bump the depth; the gate frees when the outermost lease releases.
    state: Mutex<Option<(std::thread::ThreadId, u64)>>,
    condvar: parking_lot::Condvar,
    /// Cumulative admission counters (see [`WriteGateStats`]).
    acquisitions: AtomicU64,
    wait_nanos: AtomicU64,
}

impl AutoCommitWriteGate {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(None),
            condvar: parking_lot::Condvar::new(),
            acquisitions: AtomicU64::new(0),
            wait_nanos: AtomicU64::new(0),
        })
    }

    pub(crate) fn acquire(self: &Arc<Self>) -> Arc<AutoCommitWriteLease> {
        let start = std::time::Instant::now();
        let current = std::thread::current().id();
        let mut guard = self.state.lock();
        // Re-entrant admission: the holder re-acquiring bumps the depth.
        if let Some((holder, depth)) = guard.as_mut() {
            if *holder == current {
                *depth += 1;
                drop(guard);
                self.acquisitions.fetch_add(1, Ordering::Relaxed);
                return Arc::new(AutoCommitWriteLease {
                    gate: self.clone(),
                    held: AtomicBool::new(true),
                });
            }
        }
        while guard.is_some() {
            self.condvar.wait(&mut guard);
        }
        *guard = Some((current, 1));
        drop(guard);
        self.acquisitions.fetch_add(1, Ordering::Relaxed);
        self.wait_nanos
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        Arc::new(AutoCommitWriteLease {
            gate: self.clone(),
            held: AtomicBool::new(true),
        })
    }

    pub(crate) fn stats(&self) -> WriteGateStats {
        WriteGateStats {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            wait_nanos: self.wait_nanos.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn release(&self) {
        let mut guard = self.state.lock();
        match guard.as_mut() {
            Some((_, depth)) if *depth > 1 => *depth -= 1,
            Some(_) => {
                *guard = None;
                drop(guard);
                self.condvar.notify_one();
            }
            None => {}
        }
    }
}

pub(crate) struct AutoCommitWriteLease {
    gate: Arc<AutoCommitWriteGate>,
    held: AtomicBool,
}

impl AutoCommitWriteLease {
    pub(crate) fn release(&self) {
        if self.held.swap(false, Ordering::AcqRel) {
            self.gate.release();
        }
    }
}

impl Drop for AutoCommitWriteLease {
    fn drop(&mut self) {
        if self.held.swap(false, Ordering::AcqRel) {
            self.gate.release();
        }
    }
}

/// Collects before-image undo entries and write-set entity keys while an
/// auto-commit statement executes.
pub(crate) struct AutoCommitMutationRecorder {
    pub(crate) undo: Arc<Mutex<UndoLogManager>>,
    /// Write set for conflict detection (tracks modified vertices and edges)
    pub(crate) write_set: Arc<Mutex<graphdb_transaction::types::WriteSet>>,
}

impl std::fmt::Debug for AutoCommitMutationRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoCommitMutationRecorder").finish()
    }
}

impl graphdb_transaction::TransactionMutationRecorder for AutoCommitMutationRecorder {
    fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        for entity_key in mutation.entity_keys {
            match entity_key {
                MutationEntityKey::Vertex(vertex_id) => self.record_vertex_write(vertex_id),
                MutationEntityKey::Edge(edge) => self.record_edge_write(edge),
            }
        }
        if let Some(entry) = mutation.undo_entry {
            self.add_undo_log(entry)
        } else {
            Ok(())
        }
    }

    fn record_vertex_write(&self, vertex_id: VertexId) {
        self.write_set.lock().record_vertex(vertex_id);
    }

    fn record_vertex_delete(&self, vertex_id: VertexId) {
        self.write_set.lock().record_vertex_delete(vertex_id);
    }

    fn record_edge_write(&self, edge: EdgeIdentifier) {
        self.write_set.lock().record_edge(edge);
    }

    fn add_undo_log(&self, entry: UndoLogEntry) -> Result<(), TransactionError> {
        self.undo
            .lock()
            .add(entry)
            .map_err(|e| TransactionError::internal(e.to_string()))
    }

    fn record_table_modification(&self, _table_name: &str) {}
}

/// A shared auto-commit batch window.
pub struct AutoCommitBatchWindow {
    pub(crate) base_ctx: Arc<super::GraphStorageContext>,
    pub(crate) gate_lease: Arc<AutoCommitWriteLease>,
    pub(crate) first_ts: Mutex<Option<Timestamp>>,
    /// Lazily registered vertex snapshots as (label, handle) pairs. One
    /// entry per registration; the per-timestamp refcount stays balanced when
    /// the same label is registered by several statements (group mode shares
    /// one timestamp).
    pub(crate) registered_vertex_snapshots: Mutex<Vec<(LabelId, SnapshotHandle)>>,
    /// Lazily registered edge-partition snapshots as (key, timestamp) pairs.
    pub(crate) registered_edge_snapshots: Mutex<Vec<(EdgeTableKey, Timestamp)>>,
    pub(crate) statement_count: AtomicU64,
    pub(crate) snapshot_rounds: AtomicU64,
    /// Group mode: statements share one write timestamp (first_ts),
    /// one undo log, and one commit point; see `begin_auto_commit_group`.
    pub(crate) group: AtomicBool,
    /// Shared before-image undo log for group mode (one segment per statement).
    pub(crate) group_undo: Option<Arc<Mutex<UndoLogManager>>>,
}

impl AutoCommitBatchWindow {
    pub(crate) fn bind_statement(self: &Arc<Self>) -> StorageResult<super::GraphStorageContext> {
        let base = &self.base_ctx;
        let is_group = self.group.load(Ordering::Acquire);
        let ts = {
            let mut first = self.first_ts.lock();
            match *first {
                Some(ts) if is_group => ts,
                Some(_) => base
                    .persistent
                    .version_manager
                    .try_next_write_timestamp()
                    .map_err(|error| StorageError::db_error(error.to_string()))?,
                None => {
                    let ts = base
                        .persistent
                        .version_manager
                        .try_next_write_timestamp()
                        .map_err(|error| StorageError::db_error(error.to_string()))?;
                    *first = Some(ts);
                    self.snapshot_rounds.fetch_add(1, Ordering::SeqCst);
                    ts
                }
            }
        };

        let transaction_id = graphdb_core::types::TransactionId::new(
            base.persistent
                .next_auto_transaction_id
                .fetch_add(1, Ordering::SeqCst),
        );
        let undo_log = if is_group {
            Arc::clone(
                self.group_undo
                    .as_ref()
                    .expect("group mode requires group_undo to be initialized"),
            )
        } else {
            Arc::new(Mutex::new(UndoLogManager::new()))
        };
        let group_undo_start = if is_group {
            Some(undo_log.lock().len())
        } else {
            None
        };
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: ts,
            write_timestamp: Some(ts),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: Arc::new(parking_lot::Mutex::new(
                    graphdb_transaction::types::WriteSet::new(),
                )),
            })),
            mvcc_vertex_snapshot_handles: Vec::new(),
            mvcc_edge_snapshot_registered: false,
            registered_vertex_labels: parking_lot::RwLock::new(std::collections::HashSet::new()),
            registered_edge_partitions: parking_lot::RwLock::new(std::collections::HashSet::new()),
            auto_commit_group_start: group_undo_start,
        };

        self.statement_count.fetch_add(1, Ordering::SeqCst);
        let mut bound = (**base).clone();
        bound.operation_context = Some(Arc::new(context));
        bound.write_timestamp_lease = Some(Arc::new(super::WriteTimestampLease {
            version_manager: base.persistent.version_manager.clone(),
            timestamp: ts,
            finalized: AtomicBool::new(false),
        }));
        bound.write_gate_lease = None;
        bound.auto_commit_undo = Some(undo_log);
        bound.auto_commit_window = Some(self.clone());
        Ok(bound)
    }

    pub fn finalize(&self) -> StorageResult<()> {
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    pub fn statement_count(&self) -> u64 {
        self.statement_count.load(Ordering::SeqCst)
    }

    pub fn snapshot_rounds(&self) -> u64 {
        self.snapshot_rounds.load(Ordering::SeqCst)
    }

    pub fn is_grouped(&self) -> bool {
        self.group.load(Ordering::Acquire)
    }

    /// Single group commit point: one fsync, then barrier advance, then the
    /// shared write-timestamp commit. Order: durability → visibility.
    pub fn finalize_group(&self) -> StorageResult<()> {
        // 1) Durability: one sync covering every no-wait appended statement.
        if let Some(persistence) = self.base_ctx.persistent.persistence.as_ref() {
            if let Some(wal) = persistence.read().wal_manager() {
                wal.read().sync()?;
                let durable = wal.read().durable_lsn();
                self.base_ctx
                    .persistent
                    .index_data_manager
                    .read()
                    .advance_barriers(graphdb_core::types::CommitLsn::new(durable.as_u64()));
            }
        }
        // 2) Visibility: commit the shared write timestamp once.
        if let Some(ts) = *self.first_ts.lock() {
            self.base_ctx
                .persistent
                .version_manager
                .commit_write_timestamp(ts);
        }
        // 3) Window cleanup (unchanged): unregister snapshots, release gate.
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    /// Roll back every statement bound to this group window: execute the
    /// shared undo log against the base context, abort the shared write
    /// timestamp, and release snapshots and the write gate.
    ///
    /// Only meaningful in group mode; batch windows have per-statement undo
    /// logs owned by their bound statements.
    pub fn rollback_group(&self) -> StorageResult<()> {
        if let Some(undo) = &self.group_undo {
            let mut manager = undo.lock();
            let start_ts = self.first_ts.lock().unwrap_or(0);
            manager
                .execute_undo(&*self.base_ctx, start_ts)
                .map_err(|e| StorageError::db_error(e.to_string()))?;
            drop(manager);
            if let Some(ts) = *self.first_ts.lock() {
                self.base_ctx.abort_write_timestamp(ts);
            }
        }
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    fn unregister_snapshots(&self) {
        // Unregister every vertex snapshot registered lazily by the window's
        // statements. Each entry matches one registration (handle), so the
        // per-timestamp refcounts return to zero exactly.
        let vertex_registrations = std::mem::take(&mut *self.registered_vertex_snapshots.lock());
        if !vertex_registrations.is_empty() {
            let tables = self
                .base_ctx
                .persistent
                .data_store
                .with_vertex_tables(|tables| tables.values().cloned().collect::<Vec<_>>());
            for (label_id, handle) in vertex_registrations {
                for table in &tables {
                    if table.label() == label_id {
                        let _ = table.unregister_snapshot(handle);
                        break;
                    }
                }
            }
        }

        let edge_registrations = std::mem::take(&mut *self.registered_edge_snapshots.lock());
        if !edge_registrations.is_empty() {
            let edge_tables = self
                .base_ctx
                .persistent
                .data_store
                .with_edge_tables(|tables| tables.clone());
            for (key, ts) in edge_registrations {
                if let Some(edge_table) = edge_tables.get(&key) {
                    edge_table.write().unregister_snapshot(ts);
                }
            }
        }
    }
}

impl Drop for AutoCommitBatchWindow {
    fn drop(&mut self) {
        self.unregister_snapshots();
    }
}

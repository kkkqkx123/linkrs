use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use linkrs_core::types::EdgeIdentifier;
use linkrs_core::types::Timestamp;
use linkrs_core::{StorageError, StorageResult};
use linkrs_transaction::undo_log::UndoLogManager;
use linkrs_transaction::{
    MutationEntityKey, MutationResult, TransactionError, UndoLogEntry, VertexId,
};

use super::GraphStorageContext;
use crate::StorageOperationContext;

/// Cumulative gate admission statistics (acquisitions and total wait time).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriteGateStats {
    /// Number of gate acquisitions (statements serialized).
    pub acquisitions: u64,
    /// Total time spent waiting for admission, in nanoseconds.
    pub wait_nanos: u64,
}

impl WriteGateStats {
    /// Gate-wait share of total thread time since `before`.
    ///
    /// Divides the waited nanos by `wall * threads` (the same caliber the
    /// `write_gate_bench` reports). Returns `0.0` for empty runs.
    pub fn share_since(
        &self,
        before: &WriteGateStats,
        wall: std::time::Duration,
        threads: usize,
    ) -> f64 {
        let waited = self.wait_nanos.saturating_sub(before.wait_nanos) as f64;
        let budget = wall.as_nanos() as f64 * threads.max(1) as f64;
        if budget <= 0.0 {
            0.0
        } else {
            waited / budget
        }
    }

    /// Whether callers should prefer batch commits over per-statement
    /// auto-commit for the observed share.
    ///
    /// Usage governance (no engine change): at or above 20% gate-wait share
    /// the global `AutoCommitWriteGate` serializes the workload, so one gate
    /// acquisition per statement wastes most thread time waiting. Prefer
    /// `batch_insert_edges` (one commit for the whole batch) or the
    /// `begin_auto_commit_group` window (one gate lease for many statements)
    /// instead of adding finer locks below the gate.
    pub fn prefer_batch_commits(share: f64) -> bool {
        share >= 0.20
    }

    /// Human-readable contention advice for the observed share.
    pub fn contention_advice(share: f64) -> &'static str {
        if Self::prefer_batch_commits(share) {
            "gate-wait share above 20%: prefer batch_insert_edges or begin_auto_commit_group over per-statement auto-commit; finer locks below the gate are not justified"
        } else {
            "gate-wait share below 20%: per-statement auto-commit is fine; keep explicit caller chunking"
        }
    }
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
    pub(crate) write_set: Arc<Mutex<linkrs_transaction::types::WriteSet>>,
}

impl std::fmt::Debug for AutoCommitMutationRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoCommitMutationRecorder").finish()
    }
}

impl linkrs_transaction::TransactionMutationRecorder for AutoCommitMutationRecorder {
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
    pub(crate) statement_count: AtomicU64,
    pub(crate) snapshot_rounds: AtomicU64,
    /// Group mode: statements share one write timestamp (first_ts),
    /// one undo log, and one commit point; see `begin_auto_commit_group`.
    pub(crate) group: AtomicBool,
    /// Shared before-image undo log for group mode (one segment per statement).
    pub(crate) group_undo: Option<Arc<Mutex<UndoLogManager>>>,
    /// One transaction id shared by every statement of a group window: the
    /// group's staging buffer and staged WAL are keyed by it, and the group
    /// commit point applies and flushes them together.
    pub(crate) group_transaction_id: Mutex<Option<linkrs_core::types::TransactionId>>,
    /// Accumulated statement write sets for group mode. Each grouped
    /// statement pushes its write set at finalize; the group commit point
    /// merges and publishes them so later transactions certify against the
    /// grouped commit.
    pub(crate) group_write_sets: Mutex<Vec<linkrs_transaction::types::WriteSet>>,
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

        let transaction_id = if is_group {
            let mut shared = self.group_transaction_id.lock();
            if shared.is_none() {
                *shared = Some(linkrs_core::types::TransactionId::new(
                    base.persistent
                        .next_auto_transaction_id
                        .fetch_add(1, Ordering::SeqCst),
                ));
            }
            *shared.as_ref().expect("group transaction id is set above")
        } else {
            linkrs_core::types::TransactionId::new(
                base.persistent
                    .next_auto_transaction_id
                    .fetch_add(1, Ordering::SeqCst),
            )
        };
        // Grouped statements share one staging buffer and one staged WAL,
        // so the statement boundary is a (journal, index, wal-length) mark
        // used to rewind exactly this statement on failure.
        let (staging_start, wal_start) = if is_group {
            (
                Some(base.peek_txn_staging_mark(transaction_id).unwrap_or((0, 0))),
                base.staged_wal_len_for(transaction_id),
            )
        } else {
            (None, 0)
        };
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
        let write_set = Arc::new(parking_lot::Mutex::new(
            linkrs_transaction::types::WriteSet::new(),
        ));
        let context = StorageOperationContext {
            transaction_id: Some(transaction_id),
            read_timestamp: ts,
            write_timestamp: Some(ts),
            read_only: false,
            auto_commit: true,
            mutation_recorder: Some(Arc::new(AutoCommitMutationRecorder {
                undo: undo_log.clone(),
                write_set: write_set.clone(),
            })),
            auto_commit_group_start: group_undo_start,
            auto_commit_staging_start: staging_start,
            auto_commit_wal_start: wal_start,
        };

        self.statement_count.fetch_add(1, Ordering::SeqCst);
        let mut bound = (**base).clone();
        bound.operation_context = Some(Arc::new(context));
        // Group mode shares one write timestamp across statements: no
        // per-statement lease, otherwise dropping a bound statement would
        // abort the shared timestamp before the group commit point settles
        // it. The group commit/rollback point owns the settle exactly once.
        bound.write_timestamp_lease = if is_group {
            None
        } else {
            Some(Arc::new(super::WriteTimestampLease {
                version_manager: base.persistent.version_manager.clone(),
                timestamp: ts,
                finalized: AtomicBool::new(false),
            }))
        };
        bound.write_gate_lease = None;
        bound.auto_commit_undo = Some(undo_log);
        bound.auto_commit_write_set = Some(write_set);
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
    /// The shared timestamp settles through the commit-ordered path so group
    /// commits share the commit-stamp coordinate with explicit transactions,
    /// and the accumulated window write set is published for later
    /// certification.
    pub fn finalize_group(&self) -> StorageResult<()> {
        // 0) Commit apply: the group's staged vertex rows install into the
        // main tables, then the accumulated WAL redo is appended as one
        // transaction (no fsync yet). A failure here aborts the whole group.
        let group_txid = *self.group_transaction_id.lock();
        if let Some(txid) = group_txid {
            if let Err(error) = self.base_ctx.apply_txn_staging(txid) {
                let _ = self.rollback_group();
                return Err(error);
            }
            if let Err(error) = self.base_ctx.commit_staged_writes_grouped(txid, &[]) {
                let _ = self.rollback_group();
                return Err(error);
            }
        }
        // 1) Durability: one sync covering the appended group redo.
        if let Some(persistence) = self.base_ctx.persistent.persistence.as_ref() {
            if let Some(wal) = persistence.read().wal_manager() {
                wal.read().sync()?;
                let durable = wal.read().durable_lsn();
                self.base_ctx
                    .persistent
                    .index_data_manager
                    .read()
                    .advance_barriers(linkrs_core::types::CommitLsn::new(durable.as_u64()));
            }
        }
        // 2) Visibility: commit the shared write timestamp once, in commit
        // order, then publish the merged window write set.
        if let Some(ts) = *self.first_ts.lock() {
            let commit_ts = self.base_ctx.commit_write_timestamp_ordered(ts)?;
            let sets = std::mem::take(&mut *self.group_write_sets.lock());
            if !sets.is_empty() {
                let mut merged = linkrs_transaction::types::WriteSet::new();
                for set in sets {
                    merged.vertices.extend(set.vertices);
                    merged.edges.extend(set.edges);
                    merged.edge_endpoints.extend(set.edge_endpoints);
                    merged.deleted_vertices.extend(set.deleted_vertices);
                    merged.schema_resources.extend(set.schema_resources);
                    merged.index_resources.extend(set.index_resources);
                    merged.read_ranges.extend(set.read_ranges);
                }
                if !merged.is_empty() {
                    self.base_ctx.publish_committed_write_set(commit_ts, merged);
                }
            }
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
        // Drop the group's staged WAL and discard the staging buffer,
        // releasing every reservation it held: the rows were never applied.
        if let Some(txid) = *self.group_transaction_id.lock() {
            self.base_ctx.drop_staged_wal_for(txid);
            self.base_ctx.discard_txn_staging_for(txid);
        }
        self.group_write_sets.lock().clear();
        self.unregister_snapshots();
        self.gate_lease.release();
        Ok(())
    }

    fn unregister_snapshots(&self) {}
}

impl Drop for AutoCommitBatchWindow {
    fn drop(&mut self) {
        self.unregister_snapshots();
    }
}

impl GraphStorageContext {
    pub(crate) fn begin_auto_commit_batch(
        &self,
    ) -> StorageResult<Arc<super::AutoCommitBatchWindow>> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let mut clean = self.clone();
        clean.operation_context = None;
        clean.write_timestamp_lease = None;
        clean.write_gate_lease = None;
        clean.auto_commit_undo = None;
        clean.auto_commit_write_set = None;
        clean.auto_commit_window = None;
        Ok(Arc::new(super::AutoCommitBatchWindow {
            base_ctx: Arc::new(clean),
            gate_lease: write_gate_lease,
            first_ts: parking_lot::Mutex::new(None),
            statement_count: std::sync::atomic::AtomicU64::new(0),
            snapshot_rounds: std::sync::atomic::AtomicU64::new(0),
            group: std::sync::atomic::AtomicBool::new(false),
            group_undo: None,
            group_write_sets: parking_lot::Mutex::new(Vec::new()),
            group_transaction_id: parking_lot::Mutex::new(None),
        }))
    }

    pub(crate) fn begin_auto_commit_group(
        &self,
    ) -> StorageResult<Arc<super::AutoCommitBatchWindow>> {
        let write_gate_lease = self.persistent.auto_commit_write_gate.acquire();
        let mut clean = self.clone();
        clean.operation_context = None;
        clean.write_timestamp_lease = None;
        clean.write_gate_lease = None;
        clean.auto_commit_undo = None;
        clean.auto_commit_write_set = None;
        clean.auto_commit_window = None;
        Ok(Arc::new(super::AutoCommitBatchWindow {
            base_ctx: Arc::new(clean),
            gate_lease: write_gate_lease,
            first_ts: parking_lot::Mutex::new(None),
            statement_count: std::sync::atomic::AtomicU64::new(0),
            snapshot_rounds: std::sync::atomic::AtomicU64::new(0),
            group: std::sync::atomic::AtomicBool::new(true),
            group_undo: Some(Arc::new(parking_lot::Mutex::new(
                linkrs_transaction::UndoLogManager::new(),
            ))),
            group_write_sets: parking_lot::Mutex::new(Vec::new()),
            group_transaction_id: parking_lot::Mutex::new(None),
        }))
    }

    pub(crate) fn restore_auto_transaction_id(&self, max_transaction_id: u64) {
        self.persistent
            .next_auto_transaction_id
            .fetch_max(max_transaction_id.saturating_add(1), Ordering::SeqCst);
    }

    pub(crate) fn abort_write_timestamp(&self, timestamp: Timestamp) {
        if let Some(lease) = &self.write_timestamp_lease {
            lease.abort();
        } else if self.operation_context.is_none() {
            self.persistent
                .version_manager
                .abort_write_timestamp(timestamp);
        }
    }

    /// Settle an auto-commit write timestamp in commit order.
    ///
    /// Reserves a commit timestamp for `start` and publishes visibility
    /// over both slots, so auto-commit statements share the
    /// commit-ordered coordinate with explicit transactions (conflict
    /// windows and the read frontier alike). Returns the commit
    /// timestamp for conflict-index publication. Re-settling an
    /// already-settled slot is a benign no-op success reporting `start`:
    /// writer helpers and operation finalization settle the same timestamp
    /// by construction (per-statement commits stay visible to later
    /// statements while the finalizer still settles), and the lease
    /// `finalized` flag already carries exactly-once intent. Ordering is
    /// guaranteed for the first settle of an acquired slot; the strict
    /// fail-closed check lives in `VersionManager::commit_ordered`.
    pub(crate) fn commit_write_timestamp_ordered(
        &self,
        start: Timestamp,
    ) -> StorageResult<Timestamp> {
        let version_manager = &self.persistent.version_manager;
        let commit_ts = match version_manager.commit_ordered(start) {
            Ok(commit_ts) => commit_ts,
            Err(linkrs_transaction::VersionManagerError::InvalidTimestamp(_)) => start,
            Err(error) => return Err(StorageError::db_error(error.to_string())),
        };
        if let Some(lease) = &self.write_timestamp_lease {
            lease.finalized.store(true, Ordering::SeqCst);
        }
        Ok(commit_ts)
    }

    pub(crate) fn finalize_operation(&self, committed: bool) -> StorageResult<()> {
        let Some(operation) = &self.operation_context else {
            return Ok(());
        };
        if !operation.auto_commit {
            return Ok(());
        }

        if self.auto_commit_window.is_none() {
            self.unregister_statement_snapshots(operation);
        }

        if operation.read_only {
            return Ok(());
        }

        // Group mode: per-statement finalize — certify against recently
        // committed write sets; on failure rewind only this statement's
        // staging segment and WAL tail. The group's vertex apply and WAL
        // flush happen once at `finalize_group`. Do NOT commit/abort the
        // write timestamp, release the gate, or unregister snapshots —
        // those are deferred to `finalize_group`.
        if let Some(window) = &self.auto_commit_window {
            if window.is_grouped() {
                let timestamp = operation.write_timestamp.ok_or_else(|| {
                    StorageError::db_error("Group operation has no write timestamp")
                })?;
                let rewind = |ctx: &Self| {
                    if let Some(txid) = operation.transaction_id {
                        let staging_start = operation.auto_commit_staging_start.unwrap_or((0, 0));
                        ctx.rewind_grouped_statement(
                            txid,
                            staging_start,
                            operation.auto_commit_wal_start,
                        );
                    }
                };
                if committed {
                    if let Some(conflict) = self.auto_commit_conflict(operation) {
                        if let Some(undo) = &self.auto_commit_undo {
                            let mut log = undo.lock();
                            let start = operation.auto_commit_group_start.unwrap_or(0);
                            if let Err(error) = log.execute_undo_from_index(self, timestamp, start)
                            {
                                log::error!("Group statement rollback failed: {}", error);
                            }
                        }
                        rewind(self);
                        self.maybe_run_index_gc();
                        return Err(conflict);
                    }
                    if let Some(write_set) = self.auto_commit_write_set.as_ref() {
                        let set = write_set.lock().clone();
                        if !set.is_empty() {
                            window.group_write_sets.lock().push(set);
                        }
                    }
                } else {
                    if let Some(undo) = &self.auto_commit_undo {
                        let mut log = undo.lock();
                        let start = operation.auto_commit_group_start.unwrap_or(0);
                        if let Err(error) = log.execute_undo_from_index(self, timestamp, start) {
                            log::error!("Group statement rollback failed: {}", error);
                        }
                    }
                    rewind(self);
                }
                self.maybe_run_index_gc();
                return Ok(());
            }
        }

        let timestamp = operation.write_timestamp.ok_or_else(|| {
            StorageError::db_error("Auto-commit operation has no write timestamp")
        })?;
        let transaction_id = operation.transaction_id;

        if committed {
            if let Some(conflict) = self.auto_commit_conflict(operation) {
                if let Some(undo) = &self.auto_commit_undo {
                    let mut log = undo.lock();
                    if let Err(error) = log.execute_undo(self, timestamp) {
                        log::error!("Auto-commit rollback failed: {}", error);
                    }
                }
                self.abort_write_timestamp(timestamp);
                if let Some(lease) = &self.write_gate_lease {
                    lease.release();
                }
                if let Some(transaction_id) = transaction_id {
                    self.abort_staged_writes(transaction_id);
                }
                self.maybe_run_index_gc();
                return Err(conflict);
            }
            // Commit point: staged vertex rows are installed after the
            // conflict certification and before the timestamp publishes
            // visibility. A failing apply leaves nothing installed; the
            // statement then unwinds like any other conflict.
            if let Some(transaction_id) = transaction_id {
                if let Err(error) = self.apply_txn_staging(transaction_id) {
                    if let Some(undo) = &self.auto_commit_undo {
                        let mut log = undo.lock();
                        if let Err(undo_error) = log.execute_undo(self, timestamp) {
                            log::error!("Auto-commit rollback failed: {}", undo_error);
                        }
                    }
                    self.abort_write_timestamp(timestamp);
                    if let Some(lease) = &self.write_gate_lease {
                        lease.release();
                    }
                    self.abort_staged_writes(transaction_id);
                    self.maybe_run_index_gc();
                    return Err(error);
                }
            }
            // Commit-ordered visibility: the conflict window below is
            // indexed by the same commit timestamp that advances the
            // read frontier, matching explicit transactions.
            let commit_ts = self.commit_write_timestamp_ordered(timestamp)?;
            self.publish_auto_commit_write_set(commit_ts);
        } else {
            if let Some(undo) = &self.auto_commit_undo {
                let mut log = undo.lock();
                if let Err(error) = log.execute_undo(self, timestamp) {
                    log::error!("Auto-commit rollback failed: {}", error);
                }
            }
            self.abort_write_timestamp(timestamp);
        }
        if let Some(lease) = &self.write_gate_lease {
            lease.release();
        }
        if let Some(transaction_id) = transaction_id {
            self.persistent.staged_wal.remove(&transaction_id);
            // After a successful apply the buffer is already gone; this is
            // the release point for the failed-statement buffer.
            self.discard_txn_staging_for(transaction_id);
        }
        self.maybe_run_index_gc();
        Ok(())
    }

    /// Check the active auto-commit statement against recently committed
    /// write sets. Returns a write-write conflict error when the statement
    /// overlaps a commit newer than its read timestamp.
    fn auto_commit_conflict(&self, operation: &StorageOperationContext) -> Option<StorageError> {
        let write_set = self.auto_commit_write_set.as_ref()?.lock().clone();
        if write_set.is_empty() {
            return None;
        }
        if self
            .persistent
            .committed_write_sets
            .has_conflict(&write_set, operation.read_timestamp)
        {
            return Some(StorageError::write_write_conflict(format!(
                "auto-commit statement at ts={} overlaps a recently committed write",
                operation.read_timestamp,
            )));
        }
        None
    }

    /// Test oracle for group-commit publication: whether `write_set`
    /// overlaps a committed entry newer than `read_ts`.
    #[cfg(test)]
    pub(crate) fn committed_write_conflict_probe(
        &self,
        write_set: &linkrs_transaction::types::WriteSet,
        read_ts: Timestamp,
    ) -> bool {
        self.persistent
            .committed_write_sets
            .has_conflict(write_set, read_ts)
    }

    /// Publish the active auto-commit statement's write set for future
    /// commit-time certification.
    fn publish_auto_commit_write_set(&self, commit_ts: Timestamp) {
        let Some(write_set) = self.auto_commit_write_set.as_ref() else {
            return;
        };
        let write_set = write_set.lock().clone();
        self.publish_committed_write_set(commit_ts, write_set);
    }

    /// Publish an externally committed write set (explicit-transaction
    /// bridge). Lets the transaction manager feed explicit commits into the
    /// storage certification window so auto-commit statements certify
    /// against them.
    pub(crate) fn publish_committed_write_set(
        &self,
        commit_ts: Timestamp,
        write_set: linkrs_transaction::types::WriteSet,
    ) {
        let horizon = self
            .persistent
            .version_manager
            .snapshot_tracker()
            .cleanup_threshold();
        self.persistent
            .committed_write_sets
            .publish(commit_ts, write_set, horizon);
    }

    fn unregister_statement_snapshots(&self, _operation: &StorageOperationContext) {}
}

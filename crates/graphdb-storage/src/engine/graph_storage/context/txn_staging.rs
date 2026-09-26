//! Transaction-level vertex write staging.
//!
//! Online vertex writes accumulate in a per-transaction buffer until the
//! transaction's commit apply installs them into the main tables. The
//! buffer shares the lifecycle of the per-transaction staged WAL: created
//! lazily on first stage, dropped together with the staged WAL at commit
//! publish or rollback, and never persisted (a crash discards it).
//!
//! `WriteScope` stays the single-request staging shape (primary-key dedup
//! and one-timestamp constraint inside one call); at statement end its rows
//! are merged into this buffer under the cross-statement overwrite rules:
//! insert-then-delete cancels both, delete-then-insert folds into a
//! whole-row update of the existing row, updates fold into a staged insert
//! row, and a delete swallows earlier updates. All merging lives here; the
//! main tables never see an uncommitted byte.
//!
//! Rows are keyed by external [`IdKey`] so a staged insert (which has no
//! internal id yet) can be targeted by a later update or delete of the same
//! transaction. External-key resolutions are cached at stage time from a
//! read-only snapshot; the cache is valid only inside this transaction.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use graphdb_core::types::{LabelId, Timestamp, TransactionId};
use graphdb_core::{StorageError, StorageResult, Value};

use crate::vertex::{IdKey, ShardedVertexTable, WriteScope};

/// Upper bound for staged rows held by one transaction.
///
/// Staging is memory-only with no spill: past this count a stage fails
/// with `capacity_exceeded` instead of silently falling back to a direct
/// write. The per-request bound in [`crate::vertex::MAX_WRITE_SCOPE_KEYS`]
/// still applies inside a single statement.
pub const MAX_TXN_STAGING_KEYS: usize = 1 << 20;

/// Cross-statement vertex write staging owned by one transaction.
///
/// Staged insert rows carry the reserved global vertex id obtained at
/// stage time, so dependent writes (edge endpoints) can reference the row
/// before the main table binds the key. Statement boundaries take a
/// [`TxnStagingMark`]; [`Self::rollback_to`] restores the buffer to a mark
/// and reports the reservations that left or re-entered the buffer so the
/// caller can settle them against the shard id managers.
#[derive(Debug)]
pub(crate) struct TxnStaging {
    write_ts: Timestamp,
    inserts: HashMap<(LabelId, IdKey), (u32, Vec<(String, Value)>)>,
    updates: HashMap<(LabelId, IdKey), Vec<(String, Value)>>,
    deletes: HashSet<(LabelId, IdKey)>,
    resolved: HashMap<(LabelId, IdKey), u32>,
    journal: Vec<UndoOp>,
    index_ops: Vec<StagedIndexOp>,
}

/// Index maintenance recorded by an online writer and executed at commit
/// apply, after the row bytes land. Replay in stage order reproduces exactly
/// what statement-time maintenance would have written, so a rolled-back
/// transaction leaves no index residue: its operations never ran.
#[derive(Debug)]
pub(crate) enum StagedIndexOp {
    Insert {
        space_id: u64,
        vid: Value,
        tag: String,
        properties: Vec<(String, Value)>,
    },
    Update {
        space_id: u64,
        vid: Value,
        tag: String,
        properties: Vec<(String, Value)>,
    },
    Delete {
        space_id: u64,
        vid: Value,
        tag: String,
    },
}

/// Journal position marking the start of one statement's buffer mutations.
/// The key-resolution cache is monotonic inside the transaction (the main
/// tables see no uncommitted binding changes) and never rolls back.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TxnStagingMark {
    journal_len: usize,
    index_len: usize,
}

impl TxnStagingMark {
    /// Flat form for storage on the operation context.
    pub(crate) fn to_lengths(&self) -> (usize, usize) {
        (self.journal_len, self.index_len)
    }

    pub(crate) fn from_lengths(journal_len: usize, index_len: usize) -> Self {
        Self {
            journal_len,
            index_len,
        }
    }
}

/// One journalled buffer map mutation and its prior entry.
#[derive(Debug)]
enum UndoOp {
    Inserts {
        pair: (LabelId, IdKey),
        prev: Option<(u32, Vec<(String, Value)>)>,
    },
    Updates {
        pair: (LabelId, IdKey),
        prev: Option<Vec<(String, Value)>>,
    },
    Deletes {
        pair: (LabelId, IdKey),
        prev_present: bool,
    },
}

/// Outcome of [`TxnStaging::rollback_to`]: `(label, reserved id)` pairs the
/// buffer no longer holds (caller releases them) and re-entries of restated
/// rows (label, key, id) whose reservation must be claimed again by the
/// caller, re-reserving through the owning table when the slot was meanwhile
/// taken. Ids are label-paired because global ids are unique only inside
/// one label's table.
#[derive(Debug, Default)]
pub(crate) struct StagingRollback {
    pub(crate) released: Vec<(LabelId, u32)>,
    pub(crate) reclaimed: Vec<(LabelId, IdKey, u32)>,
}

impl TxnStaging {
    pub(crate) fn new(write_ts: Timestamp) -> Self {
        Self {
            write_ts,
            inserts: HashMap::new(),
            updates: HashMap::new(),
            deletes: HashSet::new(),
            resolved: HashMap::new(),
            journal: Vec::new(),
            index_ops: Vec::new(),
        }
    }

    /// The single write timestamp every row of this staging will land with.
    pub(crate) fn write_ts(&self) -> Timestamp {
        self.write_ts
    }

    /// Reject staging under a different timestamp.
    pub(crate) fn ensure_same_write_ts(&self, ts: Timestamp) -> StorageResult<()> {
        if self.write_ts != ts {
            return Err(StorageError::invalid_operation(format!(
                "transaction staging created for ts={} cannot serve ts={}",
                self.write_ts, ts
            )));
        }
        Ok(())
    }

    pub(crate) fn len(&self) -> usize {
        self.inserts.len() + self.updates.len() + self.deletes.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }

    // ── key resolution cache ──────────────────────────────────────────────

    /// Cache an external-key to internal-id resolution taken from a
    /// read-only snapshot at stage time.
    pub(crate) fn cache_resolution(&mut self, label: LabelId, key: IdKey, id: u32) {
        self.resolved.insert((label, key), id);
    }

    pub(crate) fn resolve(&self, label: LabelId, key: &IdKey) -> Option<u32> {
        self.resolved.get(&(label, key.clone())).copied()
    }

    fn counts_as_new(&self, label: LabelId, key: &IdKey) -> bool {
        !self.inserts.contains_key(&(label, key.clone()))
            && !self.updates.contains_key(&(label, key.clone()))
            && !self.deletes.contains(&(label, key.clone()))
    }

    fn ensure_capacity(&self) -> StorageResult<()> {
        if self.len() >= MAX_TXN_STAGING_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        Ok(())
    }

    // ── staging merges ────────────────────────────────────────────────────

    fn record_inserts(&mut self, pair: (LabelId, IdKey)) {
        let prev = self.inserts.get(&pair).cloned();
        self.journal.push(UndoOp::Inserts { pair, prev });
    }

    fn record_updates(&mut self, pair: (LabelId, IdKey)) {
        let prev = self.updates.get(&pair).cloned();
        self.journal.push(UndoOp::Updates { pair, prev });
    }

    fn record_deletes(&mut self, pair: (LabelId, IdKey)) {
        let prev_present = self.deletes.contains(&pair);
        self.journal.push(UndoOp::Deletes { pair, prev_present });
    }

    /// Fold one staged insert row and its reserved global id into the
    /// transaction buffer. Returns a reserved id the buffer does not hold
    /// (a superseded reservation or an unused one on the delete-fold path)
    /// for the caller to release.
    pub(crate) fn stage_insert(
        &mut self,
        label: LabelId,
        key: IdKey,
        reserved: u32,
        properties: Vec<(String, Value)>,
    ) -> StorageResult<Option<u32>> {
        let pair = (label, key.clone());
        if self.counts_as_new(label, &key) {
            self.ensure_capacity()?;
        }
        if self.deletes.contains(&pair) {
            // delete-then-insert folds into a whole-row update of the
            // existing row; the incoming reservation names that already
            // bound row, so the buffer never holds it.
            self.record_deletes(pair.clone());
            self.deletes.remove(&pair);
            self.record_updates(pair.clone());
            self.updates.insert(pair, properties);
            return Ok(Some(reserved));
        }
        let superseded = self
            .inserts
            .get(&pair)
            .filter(|(held, _)| *held != reserved)
            .map(|(held, _)| *held);
        self.record_inserts(pair.clone());
        self.inserts.insert(pair, (reserved, properties));
        Ok(superseded)
    }

    /// Fold staged column updates into the transaction buffer.
    pub(crate) fn stage_update(
        &mut self,
        label: LabelId,
        key: IdKey,
        columns: Vec<(String, Value)>,
    ) -> StorageResult<()> {
        let pair = (label, key.clone());
        if self.inserts.contains_key(&pair) {
            // An update of a row this transaction inserted merges into the
            // staged insert row; the main table never sees the update.
            self.record_inserts(pair.clone());
            let row = &mut self.inserts.get_mut(&pair).expect("checked").1;
            for (name, value) in columns {
                match row.iter_mut().find(|(existing, _)| *existing == name) {
                    Some(slot) => slot.1 = value,
                    None => row.push((name, value)),
                }
            }
            return Ok(());
        }
        if self.counts_as_new(label, &key) {
            self.ensure_capacity()?;
        }
        self.record_updates(pair.clone());
        self.updates.entry(pair).or_default().extend(columns);
        Ok(())
    }

    /// Stage a delete of the identified row. Returns the reserved id of a
    /// cancelled insert row for the caller to release.
    pub(crate) fn stage_delete(
        &mut self,
        label: LabelId,
        key: IdKey,
    ) -> StorageResult<Option<u32>> {
        let pair = (label, key.clone());
        if self.inserts.contains_key(&pair) {
            // insert-then-delete cancels: the row never reaches the main
            // table, so its reservation leaves the buffer. The journal
            // captures the full row ahead of removal for statement
            // rollback to restore it.
            self.record_inserts(pair.clone());
            let cancelled = self.inserts.remove(&pair).map(|(reserved, _)| reserved);
            if self.updates.contains_key(&pair) {
                self.record_updates(pair.clone());
                self.updates.remove(&pair);
            }
            return Ok(cancelled);
        }
        // A delete swallows earlier updates of the same row.
        if self.updates.contains_key(&pair) {
            self.record_updates(pair.clone());
            self.updates.remove(&pair);
        } else if !self.deletes.contains(&pair) {
            self.ensure_capacity()?;
        }
        self.record_deletes(pair.clone());
        self.deletes.insert(pair);
        Ok(None)
    }

    // ── read-facade probes ────────────────────────────────────────────────

    pub(crate) fn has_pending_delete(&self, label: LabelId, key: &IdKey) -> bool {
        self.deletes.contains(&(label, key.clone()))
    }

    /// The reserved global id a staged insert row will commit under.
    pub(crate) fn pending_insert_id(&self, label: LabelId, key: &IdKey) -> Option<u32> {
        self.inserts.get(&(label, key.clone())).map(|(id, _)| *id)
    }

    /// A staged insert row as `(reserved id, properties)`.
    pub(crate) fn pending_insert_row(
        &self,
        label: LabelId,
        key: &IdKey,
    ) -> Option<(u32, &[(String, Value)])> {
        self.inserts
            .get(&(label, key.clone()))
            .map(|(id, props)| (*id, props.as_slice()))
    }

    /// Point a restored reservation at a freshly reserved id after a
    /// failed reclaim. Returns the id the buffer held before.
    pub(crate) fn replace_insert_id(
        &mut self,
        label: LabelId,
        key: &IdKey,
        new_id: u32,
    ) -> Option<u32> {
        self.inserts
            .get_mut(&(label, key.clone()))
            .map(|(id, _)| std::mem::replace(id, new_id))
    }

    pub(crate) fn pending_update(&self, label: LabelId, key: &IdKey) -> Option<&[(String, Value)]> {
        self.updates.get(&(label, key.clone())).map(Vec::as_slice)
    }

    /// Staged insert rows of one label (external key, reserved id,
    /// properties).
    pub(crate) fn insert_rows(
        &self,
        label: LabelId,
    ) -> impl Iterator<Item = (&IdKey, u32, &[(String, Value)])> {
        self.inserts
            .iter()
            .filter(move |pair| pair.0 .0 == label)
            .map(|((_, key), (reserved, props))| (key, *reserved, props.as_slice()))
    }

    /// Staged update rows of one label (external key, columns).
    pub(crate) fn update_rows(
        &self,
        label: LabelId,
    ) -> impl Iterator<Item = (&IdKey, &[(String, Value)])> {
        self.updates
            .iter()
            .filter(move |pair| pair.0 .0 == label)
            .map(|((_, key), cols)| (key, cols.as_slice()))
    }

    /// Staged delete keys of one label.
    pub(crate) fn delete_keys(&self, label: LabelId) -> impl Iterator<Item = &IdKey> {
        self.deletes
            .iter()
            .filter(move |pair| pair.0 == label)
            .map(|(_, key)| key)
    }

    // ── statement journal ─────────────────────────────────────────────────

    /// Journal an index maintenance operation for commit-time replay.
    pub(crate) fn stage_index_op(&mut self, op: StagedIndexOp) {
        self.index_ops.push(op);
    }

    /// Mark the current journal length as a statement boundary.
    pub(crate) fn mark(&self) -> TxnStagingMark {
        TxnStagingMark {
            journal_len: self.journal.len(),
            index_len: self.index_ops.len(),
        }
    }

    /// Undo every buffered mutation after `mark`, in reverse order, and
    /// report the net reservation movement so the caller can settle it
    /// against the shard id managers: ids the buffer no longer holds (to
    /// release) and ids a restored entry claims again (to reclaim,
    /// re-reserving through the owning table when the slot was meanwhile
    /// taken).
    pub(crate) fn rollback_to(&mut self, mark: TxnStagingMark) -> StagingRollback {
        let before: HashSet<(LabelId, u32)> = self
            .inserts
            .iter()
            .map(|((label, _), (reserved, _))| (*label, *reserved))
            .collect();
        for op in self.journal.drain(mark.journal_len..).rev() {
            match op {
                UndoOp::Inserts { pair, prev } => {
                    self.inserts.remove(&pair);
                    if let Some(entry) = prev {
                        self.inserts.insert(pair, entry);
                    }
                }
                UndoOp::Updates { pair, prev } => {
                    self.updates.remove(&pair);
                    if let Some(columns) = prev {
                        self.updates.insert(pair, columns);
                    }
                }
                UndoOp::Deletes { pair, prev_present } => {
                    if prev_present {
                        self.deletes.insert(pair);
                    } else {
                        self.deletes.remove(&pair);
                    }
                }
            }
        }
        self.index_ops.truncate(mark.index_len);
        let after: HashSet<(LabelId, u32)> = self
            .inserts
            .iter()
            .map(|((label, _), (reserved, _))| (*label, *reserved))
            .collect();
        let released = before
            .difference(&after)
            .copied()
            .collect::<Vec<(LabelId, u32)>>();
        let reclaimed = self
            .inserts
            .iter()
            .filter(|((label, _), (reserved, _))| !before.contains(&(*label, *reserved)))
            .map(|((label, key), (reserved, _))| (*label, key.clone(), *reserved))
            .collect();
        StagingRollback {
            released,
            reclaimed,
        }
    }

    // ── commit apply support ──────────────────────────────────────────────

    /// Labels touched by this staging (any of the three sets).
    pub(crate) fn labels(&self) -> Vec<LabelId> {
        let mut labels: Vec<LabelId> = self
            .inserts
            .keys()
            .chain(self.updates.keys())
            .chain(self.deletes.iter())
            .map(|(label, _)| *label)
            .collect();
        labels.sort_unstable();
        labels.dedup();
        labels
    }

    /// Drain one label's staged rows into a [`WriteScope`] for the point
    /// write channel apply. Insert rows carry their reserved ids; updates
    /// and deletes require their cached internal ids; a missing resolution
    /// is a staging misuse.
    pub(crate) fn take_label_scope(&mut self, label: LabelId) -> StorageResult<WriteScope> {
        let mut scoped_inserts = HashMap::new();
        for (key, entry) in self.inserts.extract_within(label) {
            scoped_inserts.insert((label, key), entry);
        }
        let mut scoped_updates = HashMap::new();
        for (key, columns) in self.updates.extract_within(label) {
            let id = self.resolved_id(label, &key)?;
            scoped_updates.insert((label, id), columns);
        }
        let mut scoped_deletes = HashSet::new();
        for key in self.deletes.extract_within(label) {
            let id = self.resolved_id(label, &key)?;
            scoped_deletes.insert((label, id));
        }
        self.journal.clear();
        Ok(WriteScope::from_staged(
            self.write_ts,
            scoped_inserts,
            scoped_updates,
            scoped_deletes,
        ))
    }

    fn resolved_id(&self, label: LabelId, key: &IdKey) -> StorageResult<u32> {
        self.resolve(label, key).ok_or_else(|| {
            StorageError::db_error(format!(
                "staged vertex row of label {label} lost its key resolution: {key:?}"
            ))
        })
    }

    /// Drain the pending index operations for commit-time replay.
    pub(crate) fn take_index_ops(&mut self) -> Vec<StagedIndexOp> {
        std::mem::take(&mut self.index_ops)
    }

    /// Discard every staged row without touching global state, returning
    /// the `(label, reserved id)` pairs of the dropped insert rows for the
    /// caller to release.
    pub(crate) fn clear(&mut self) -> Vec<(LabelId, u32)> {
        let reservations: Vec<(LabelId, u32)> = self
            .inserts
            .iter()
            .map(|((label, _), (reserved, _))| (*label, *reserved))
            .collect();
        self.inserts.clear();
        self.updates.clear();
        self.deletes.clear();
        self.resolved.clear();
        self.journal.clear();
        self.index_ops.clear();
        reservations
    }
}

/// Drain helpers scoped to one label for the staging maps and set.
trait ExtractByLabel<V> {
    fn extract_within(&mut self, label: LabelId) -> Vec<(IdKey, V)>;
}

impl<V> ExtractByLabel<V> for HashMap<(LabelId, IdKey), V> {
    fn extract_within(&mut self, label: LabelId) -> Vec<(IdKey, V)> {
        let keys: Vec<(LabelId, IdKey)> = self
            .keys()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for pair in keys {
            if let Some(value) = self.remove(&pair) {
                out.push((pair.1, value));
            }
        }
        out
    }
}

trait ExtractKeysByLabel {
    fn extract_within(&mut self, label: LabelId) -> Vec<IdKey>;
}

impl ExtractKeysByLabel for HashSet<(LabelId, IdKey)> {
    fn extract_within(&mut self, label: LabelId) -> Vec<IdKey> {
        let keys: Vec<(LabelId, IdKey)> = self
            .iter()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for pair in keys {
            if self.remove(&pair) {
                out.push(pair.1);
            }
        }
        out
    }
}

type StagingBuffers = dashmap::DashMap<TransactionId, Arc<Mutex<TxnStaging>>>;

impl super::GraphStorageContext {
    /// The staging buffer bound to the active write transaction, if any.
    /// Read composition treats `None` as "nothing staged".
    pub(crate) fn active_txn_staging(&self) -> Option<Arc<Mutex<TxnStaging>>> {
        let transaction_id = self.operation_context()?.transaction_id?;
        self.txn_staging_buffers()
            .get(&transaction_id)
            .map(|entry| entry.clone())
    }

    /// The buffer for the active write transaction, created on first use.
    /// Staging requires a transaction-bound write context; offline barrier
    /// tools keep their direct-write entries and never call this.
    pub(crate) fn txn_staging_buffer(
        &self,
        write_ts: Timestamp,
    ) -> StorageResult<Arc<Mutex<TxnStaging>>> {
        let transaction_id = self
            .operation_context()
            .and_then(|context| context.transaction_id)
            .ok_or_else(|| {
                StorageError::db_error("vertex staging requires a transaction-bound write context")
            })?;
        let buffers = self.txn_staging_buffers();
        let buffer = buffers
            .entry(transaction_id)
            .or_insert_with(|| Arc::new(Mutex::new(TxnStaging::new(write_ts))))
            .clone();
        buffer.lock().ensure_same_write_ts(write_ts)?;
        Ok(buffer)
    }

    fn txn_staging_buffers(&self) -> &Arc<StagingBuffers> {
        &self.persistent.txn_staging
    }

    // ── online staging entries ────────────────────────────────────────────

    /// Whether the active context is an online write bound to a transaction.
    /// Online vertex writes stage into the transaction buffer; contexts
    /// without a transaction binding (offline barrier tools, direct harness
    /// writes) keep the statement-scoped apply path.
    pub(crate) fn is_online_write(&self) -> bool {
        self.operation_context()
            .is_some_and(|context| context.transaction_id.is_some())
    }

    fn staging_vertex_table(&self, label: LabelId) -> StorageResult<Arc<ShardedVertexTable>> {
        self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .get(&label)
                .cloned()
                .ok_or_else(|| StorageError::label_not_found(format!("vertex label {}", label)))
        })
    }

    /// Release `(label, reserved id)` reservations a buffer dropped. Global
    /// ids are unique only inside one label's table, hence the pairing.
    pub(crate) fn release_staged_reservations(&self, entries: &[(LabelId, u32)]) {
        if entries.is_empty() {
            return;
        }
        self.persistent.data_store.with_vertex_tables(|tables| {
            for (label, id) in entries {
                if let Some(table) = tables.get(label) {
                    table.release_reserved_vertex(*id);
                }
            }
        });
    }

    /// Settle a statement rollback's reservation movement against the
    /// owning tables: release dropped ids, reclaim restored ids, and
    /// re-reserve for a restored row whose slot was meanwhile taken.
    pub(crate) fn settle_staging_rollback(
        &self,
        buffer: &Arc<Mutex<TxnStaging>>,
        rollback: StagingRollback,
    ) {
        self.release_staged_reservations(&rollback.released);
        for (label, key, id) in rollback.reclaimed {
            let Ok(table) = self.staging_vertex_table(label) else {
                continue;
            };
            if table.try_reclaim_reserved_vertex(id) {
                continue;
            }
            if let Ok(fresh) = table.reserve_vertex_id(&key) {
                buffer.lock().replace_insert_id(label, &key, fresh);
            }
        }
    }

    /// Statement boundary mark for the active transaction's buffer, taken
    /// before a compound statement stages.
    pub(crate) fn txn_staging_mark(
        &self,
        ts: Timestamp,
    ) -> StorageResult<(Arc<Mutex<TxnStaging>>, TxnStagingMark)> {
        let buffer = self.txn_staging_buffer(ts)?;
        let mark = buffer.lock().mark();
        Ok((buffer, mark))
    }

    /// Roll the active transaction's buffer back to a mark and settle the
    /// reservation movement it reports.
    pub(crate) fn rollback_staging_to(
        &self,
        buffer: &Arc<Mutex<TxnStaging>>,
        mark: TxnStagingMark,
    ) {
        let rollback = buffer.lock().rollback_to(mark);
        self.settle_staging_rollback(buffer, rollback);
    }

    /// Mark of one transaction's buffer as it stands, without creating the
    /// buffer. `None` means no buffer exists yet, whose mark is the zero
    /// lengths.
    pub(crate) fn peek_txn_staging_mark(
        &self,
        transaction_id: TransactionId,
    ) -> Option<(usize, usize)> {
        self.persistent
            .txn_staging
            .get(&transaction_id)
            .map(|buffer| buffer.lock().mark().to_lengths())
    }

    /// Rewind one grouped transaction to a statement's start: roll the
    /// buffer back to the statement mark and truncate the staged WAL (both
    /// shared across the group) back to the statement's append position.
    /// Edge undo stays with the group undo log segment.
    pub(crate) fn rewind_grouped_statement(
        &self,
        transaction_id: TransactionId,
        staging_start: (usize, usize),
        wal_start: usize,
    ) {
        if let Some(buffer) = self
            .persistent
            .txn_staging
            .get(&transaction_id)
            .map(|entry| entry.clone())
        {
            let mark = TxnStagingMark::from_lengths(staging_start.0, staging_start.1);
            let rollback = buffer.lock().rollback_to(mark);
            self.settle_staging_rollback(&buffer, rollback);
        }
        if let Some(mut entries) = self.persistent.staged_wal.get_mut(&transaction_id) {
            let keep = wal_start.min(entries.len());
            entries.truncate(keep);
        }
    }

    /// Stage one online vertex property update. An update of a row this
    /// transaction inserted folds into the staged insert row.
    pub(crate) fn stage_vertex_update(
        &self,
        label: LabelId,
        key: &IdKey,
        columns: Vec<(String, Value)>,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let table = self.staging_vertex_table(label)?;
        let columns = table.prepare_vertex_update(&columns)?;
        let buffer = self.txn_staging_buffer(ts)?;
        let mut guard = buffer.lock();
        if guard.pending_insert_id(label, key).is_some() {
            return guard.stage_update(label, key.clone(), columns);
        }
        if guard.has_pending_delete(label, key) {
            return Err(StorageError::vertex_not_found());
        }
        let id = match key {
            IdKey::Text(name) => table.get_internal_id(name, ts),
            IdKey::Int(n) => table.get_internal_id_by_i64(*n, ts),
        };
        let Some(id) = id else {
            return Err(StorageError::vertex_not_found());
        };
        guard.cache_resolution(label, key.clone(), id);
        guard.stage_update(label, key.clone(), columns)
    }

    /// Stage one online vertex delete. Cancelling a staged insert drops that
    /// row and releases its reservation.
    pub(crate) fn stage_vertex_delete(
        &self,
        label: LabelId,
        key: &IdKey,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let table = self.staging_vertex_table(label)?;
        let buffer = self.txn_staging_buffer(ts)?;
        let mut guard = buffer.lock();
        if guard.pending_insert_id(label, key).is_some() {
            let cancelled = guard.stage_delete(label, key.clone())?;
            drop(guard);
            if let Some(id) = cancelled {
                self.release_staged_reservations(&[(label, id)]);
            }
            return Ok(());
        }
        if guard.has_pending_delete(label, key) {
            return Err(StorageError::vertex_not_found());
        }
        let id = match key {
            IdKey::Text(name) => table.get_internal_id(name, ts),
            IdKey::Int(n) => table.get_internal_id_by_i64(*n, ts),
        };
        let Some(id) = id else {
            return Err(StorageError::vertex_not_found());
        };
        guard.cache_resolution(label, key.clone(), id);
        guard.stage_delete(label, key.clone())?;
        Ok(())
    }

    /// Journal an index maintenance operation for commit-time replay.
    pub(crate) fn stage_vertex_index_op(
        &self,
        ts: Timestamp,
        op: StagedIndexOp,
    ) -> StorageResult<()> {
        let buffer = self.txn_staging_buffer(ts)?;
        buffer.lock().stage_index_op(op);
        Ok(())
    }

    // ── statement absorption ──────────────────────────────────────────────

    /// Fold a finished request-scoped [`WriteScope`] into the transaction
    /// buffer under a statement mark, so a mid-statement failure restores
    /// the buffer and settles every reservation the statement held without
    /// any global write.
    pub(crate) fn absorb_write_scope(
        &self,
        scope: &mut WriteScope,
        ts: Timestamp,
    ) -> StorageResult<()> {
        if scope.is_empty() {
            return Ok(());
        }
        let buffer = self.txn_staging_buffer(ts)?;
        let mark = buffer.lock().mark();
        if let Err(error) = self.absorb_write_scope_inner(&buffer, scope) {
            self.rollback_staging_to(&buffer, mark);
            let mut held: Vec<(LabelId, u32)> = Vec::new();
            for label in scope.labels() {
                held.extend(
                    scope
                        .rollback_label(label)
                        .into_iter()
                        .map(|id| (label, id)),
                );
            }
            self.release_staged_reservations(&held);
            return Err(error);
        }
        Ok(())
    }

    fn absorb_write_scope_inner(
        &self,
        buffer: &Arc<Mutex<TxnStaging>>,
        scope: &mut WriteScope,
    ) -> StorageResult<()> {
        for label in scope.labels() {
            let table = self.staging_vertex_table(label)?;
            for (key, reserved, props) in scope.take_inserts_for_label(label) {
                let evicted = buffer
                    .lock()
                    .stage_insert(label, key.clone(), reserved, props)?;
                if let Some(id) = evicted {
                    self.release_staged_reservations(&[(label, id)]);
                }
            }
            for (id, columns) in scope.take_updates_for_label(label) {
                let key = reverse_key(&table, id)?;
                let mut guard = buffer.lock();
                guard.cache_resolution(label, key.clone(), id);
                guard.stage_update(label, key, columns)?;
            }
            for id in scope.take_deletes_for_label(label) {
                let key = reverse_key(&table, id)?;
                let mut guard = buffer.lock();
                guard.cache_resolution(label, key.clone(), id);
                let cancelled = guard.stage_delete(label, key)?;
                drop(guard);
                if let Some(cancelled) = cancelled {
                    self.release_staged_reservations(&[(label, cancelled)]);
                }
            }
        }
        Ok(())
    }

    // ── commit apply ──────────────────────────────────────────────────────

    /// Install every staged row of one transaction into the main tables
    /// through the per-label point write channel (shard-index order inside
    /// each label is the table channel's), then replay the journalled index
    /// operations and seed the record caches. A failing label undoes the
    /// labels this call already applied; the buffer keeps whatever it still
    /// holds for the caller's abort cleanup. On success the buffer is
    /// removed and the applied `(label, ids)` mapping is returned so the
    /// caller can compensate a later durability failure.
    pub(crate) fn apply_txn_staging(
        &self,
        transaction_id: TransactionId,
    ) -> StorageResult<Vec<(LabelId, Vec<u32>)>> {
        let Some(buffer) = self
            .persistent
            .txn_staging
            .get(&transaction_id)
            .map(|entry| entry.clone())
        else {
            return Ok(Vec::new());
        };
        let ts = buffer.lock().write_ts();
        // The guard must not outlive this statement: a `for` expression would
        // keep it alive across the loop body, which locks the buffer again.
        let labels = buffer.lock().labels();
        let mut applied: Vec<(LabelId, Vec<u32>)> = Vec::new();
        for label in labels {
            // Cache bookkeeping must be collected before the drain erases
            // the staged keys.
            let invalidations: Vec<(IdKey, u32)> = {
                let guard = buffer.lock();
                let mut pairs = Vec::new();
                for (key, _) in guard.update_rows(label) {
                    if let Some(id) = guard.resolve(label, key) {
                        pairs.push((key.clone(), id));
                    }
                }
                for key in guard.delete_keys(label) {
                    if let Some(id) = guard.resolve(label, key) {
                        pairs.push((key.clone(), id));
                    }
                }
                pairs
            };
            let mut scope = match buffer.lock().take_label_scope(label) {
                Ok(scope) => scope,
                Err(error) => {
                    self.undo_applied_staging(&applied);
                    return Err(error);
                }
            };
            if scope.is_empty() {
                continue;
            }
            let mapping = match self.commit_write_scope(label, &mut scope, ts) {
                Ok(mapping) => mapping,
                Err(error) => {
                    self.undo_applied_staging(&applied);
                    return Err(error);
                }
            };
            for (key, id) in &mapping {
                let external = match key {
                    IdKey::Text(name) => name.as_str().to_string(),
                    IdKey::Int(n) => n.to_string(),
                };
                self.cache_inserted_vertex_id(label, &external, *id, ts);
                match key {
                    IdKey::Int(n) => self.observe_vertex_id_i64(label, *n),
                    IdKey::Text(_) => self.observe_vertex_id_string(label),
                }
            }
            for (key, id) in invalidations {
                let external = match &key {
                    IdKey::Text(name) => name.clone(),
                    IdKey::Int(n) => n.to_string(),
                };
                self.persistent
                    .cache_manager
                    .remove_cached_vertex_id(label, &external);
                self.persistent
                    .cache_manager
                    .remove_cached_vertex(label, id);
            }
            self.mark_vertex_modified(label);
            applied.push((label, mapping.into_iter().map(|(_, id)| id).collect()));
        }
        let ops = buffer.lock().take_index_ops();
        let maintenance = self.replay_staged_index_ops(ops, ts);
        if let Err(error) = maintenance {
            self.undo_applied_staging(&applied);
            return Err(error);
        }
        self.persistent.txn_staging.remove(&transaction_id);
        Ok(applied)
    }

    fn replay_staged_index_ops(&self, ops: Vec<StagedIndexOp>, ts: Timestamp) -> StorageResult<()> {
        use crate::engine::graph_storage::writer::index_maintenance;
        for op in ops {
            match op {
                StagedIndexOp::Insert {
                    space_id,
                    vid,
                    tag,
                    properties,
                } => index_maintenance::update_vertex_indexes(
                    self,
                    self.index_metadata_manager(),
                    space_id,
                    &vid,
                    &tag,
                    &properties,
                    ts,
                )?,
                StagedIndexOp::Update {
                    space_id,
                    vid,
                    tag,
                    properties,
                } => index_maintenance::refresh_vertex_indexes(
                    self,
                    self.index_metadata_manager(),
                    space_id,
                    &vid,
                    &tag,
                    &properties,
                    ts,
                )?,
                StagedIndexOp::Delete { space_id, vid, tag } => {
                    index_maintenance::delete_vertex_indexes(
                        self,
                        self.index_metadata_manager(),
                        space_id,
                        &vid,
                        &tag,
                        ts,
                    )?
                }
            }
        }
        Ok(())
    }

    /// Undo the labels an [`Self::apply_txn_staging`] run already installed
    /// (insert channel only: update and delete residue stays invisible
    /// because the timestamp never publishes).
    pub(crate) fn undo_applied_staging(&self, applied: &[(LabelId, Vec<u32>)]) {
        for (label, ids) in applied {
            self.undo_applied_scope_inserts(*label, ids);
        }
    }

    /// Drop one transaction's buffer, releasing every reservation it held.
    pub(crate) fn discard_txn_staging_for(&self, transaction_id: TransactionId) {
        if let Some(buffer) = self
            .persistent
            .txn_staging
            .get(&transaction_id)
            .map(|entry| entry.clone())
        {
            let released = buffer.lock().clear();
            self.release_staged_reservations(&released);
        }
        self.persistent.txn_staging.remove(&transaction_id);
    }
}

/// Reverse a global id to its external key through the owning table. A
/// staged row's id is only ever reverse-resolvable when the key is bound,
/// which holds for every update and delete a request scope carries.
fn reverse_key(table: &ShardedVertexTable, global_id: u32) -> StorageResult<IdKey> {
    table.get_external_id_raw(global_id).ok_or_else(|| {
        StorageError::db_error(format!(
            "staged vertex row {global_id} lost its key resolution"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

    fn key(id: i64) -> IdKey {
        IdKey::Int(id)
    }

    fn props(name: &str) -> Vec<(String, Value)> {
        vec![("name".to_string(), Value::from(name))]
    }

    #[test]
    fn insert_then_delete_cancels_both() {
        let mut staging = TxnStaging::new(10);
        assert_eq!(
            staging.stage_insert(1, key(7), 5, props("a")).unwrap(),
            None
        );
        assert_eq!(staging.stage_delete(1, key(7)).unwrap(), Some(5));
        assert!(staging.is_empty());
        assert!(staging.pending_insert_row(1, &key(7)).is_none());
        assert!(!staging.has_pending_delete(1, &key(7)));
    }

    #[test]
    fn delete_then_insert_folds_to_row_update() {
        let mut staging = TxnStaging::new(10);
        staging.cache_resolution(1, key(7), 3);
        assert_eq!(staging.stage_delete(1, key(7)).unwrap(), None);
        // The reservation of a re-inserted existing row names the bound id
        // and never enters the buffer.
        assert_eq!(
            staging.stage_insert(1, key(7), 3, props("b")).unwrap(),
            Some(3)
        );
        assert!(staging.inserts.is_empty());
        assert!(!staging.has_pending_delete(1, &key(7)));
        assert_eq!(
            staging.pending_update(1, &key(7)).unwrap(),
            props("b").as_slice()
        );
    }

    #[test]
    fn update_merges_into_staged_insert() {
        let mut staging = TxnStaging::new(10);
        staging.stage_insert(1, key(7), 5, props("a")).unwrap();
        staging
            .stage_update(1, key(7), vec![("age".to_string(), Value::from(3))])
            .unwrap();
        let row = &staging.pending_insert_row(1, &key(7)).unwrap().1;
        assert_eq!(row.len(), 2);
        assert_eq!(staging.pending_insert_id(1, &key(7)), Some(5));
        assert!(staging.updates.is_empty());
        assert_eq!(staging.len(), 1);
    }

    #[test]
    fn delete_swallows_earlier_updates() {
        let mut staging = TxnStaging::new(10);
        staging.cache_resolution(1, key(7), 3);
        staging
            .stage_update(1, key(7), vec![("age".to_string(), Value::from(3))])
            .unwrap();
        assert_eq!(staging.stage_delete(1, key(7)).unwrap(), None);
        assert!(staging.updates.is_empty());
        assert!(staging.has_pending_delete(1, &key(7)));
        assert_eq!(staging.len(), 1);
    }

    #[test]
    fn later_write_overwrites_staged_insert() {
        let mut staging = TxnStaging::new(10);
        staging.stage_insert(1, key(7), 5, props("a")).unwrap();
        // Restating with the buffer's held id keeps it.
        assert_eq!(
            staging.stage_insert(1, key(7), 5, props("b")).unwrap(),
            None
        );
        // Restating with a fresh id supersedes the held reservation for the
        // caller to release.
        assert_eq!(
            staging.stage_insert(1, key(7), 9, props("c")).unwrap(),
            Some(5)
        );
        assert_eq!(staging.len(), 1);
        assert_eq!(staging.pending_insert_id(1, &key(7)), Some(9));
        assert_eq!(
            staging.pending_insert_row(1, &key(7)).unwrap().1,
            props("c").as_slice()
        );
    }

    #[test]
    fn rollback_to_mark_restores_entries_and_reports_ids() {
        let mut staging = TxnStaging::new(10);
        staging.stage_insert(1, key(7), 5, props("a")).unwrap();
        let mark = staging.mark();
        // Statement two cancels the insert and re-inserts with a fresh id.
        assert_eq!(staging.stage_delete(1, key(7)).unwrap(), Some(5));
        staging.stage_insert(1, key(7), 8, props("z")).unwrap();
        staging
            .stage_update(1, key(7), vec![("age".to_string(), Value::from(2))])
            .unwrap();
        let outcome = staging.rollback_to(mark);
        assert_eq!(outcome.released, vec![(1, 8)]);
        assert_eq!(outcome.reclaimed, vec![(1, key(7), 5)]);
        // The pre-statement insert row is restored intact with its id.
        assert_eq!(
            staging.pending_insert_row(1, &key(7)).unwrap().1,
            props("a").as_slice()
        );
        assert_eq!(staging.pending_insert_id(1, &key(7)), Some(5));
        assert!(staging.updates.is_empty());
        assert!(!staging.has_pending_delete(1, &key(7)));
    }

    #[test]
    fn rollback_of_new_statement_rows_empties_them() {
        let mut staging = TxnStaging::new(10);
        staging.cache_resolution(1, key(9), 4);
        let mark = staging.mark();
        staging.stage_insert(1, key(7), 5, props("a")).unwrap();
        // The cancelling delete hands id 5 straight back to the caller.
        assert_eq!(staging.stage_delete(1, key(7)).unwrap(), Some(5));
        staging.stage_delete(1, key(9)).unwrap();
        let outcome = staging.rollback_to(mark);
        // Nothing remained in the buffer at rollback time, so no net id
        // movement: id 5 was already released by the caller.
        assert!(outcome.released.is_empty());
        assert!(outcome.reclaimed.is_empty());
        assert!(staging.is_empty());
    }

    #[test]
    fn take_label_scope_resolves_ids_and_drains() {
        let mut staging = TxnStaging::new(10);
        staging.stage_insert(1, key(7), 5, props("a")).unwrap();
        staging.cache_resolution(1, key(9), 4);
        staging.stage_delete(1, key(9)).unwrap();
        let mut scope = staging.take_label_scope(1).unwrap();
        assert_eq!(scope.write_ts(), 10);
        let inserts = scope.take_inserts_for_label(1);
        assert_eq!(inserts.len(), 1);
        assert_eq!(inserts[0].1, 5);
        assert_eq!(scope.take_deletes_for_label(1), vec![4]);
        assert!(staging.is_empty());
    }

    #[test]
    fn take_label_scope_requires_resolution() {
        let mut staging = TxnStaging::new(10);
        staging.updates.insert((1, key(7)), props("a"));
        assert!(staging.take_label_scope(1).is_err());
    }

    #[test]
    fn rejects_cross_timestamp_staging() {
        let staging = TxnStaging::new(10);
        assert!(staging.ensure_same_write_ts(11).is_err());
    }
}

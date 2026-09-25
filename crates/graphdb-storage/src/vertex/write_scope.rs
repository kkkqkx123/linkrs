//! Caller-owned single-request primary-key dedup scope.
//!
//! The storage engine applies vertex rows immediately with timestamp ordering
//! and logical undo. The scope object carries the per-request ownership that
//! the shard and index layers must never retain: every uncommitted primary
//! key lives in the caller's buffer, shards only forward it.
//!
//! Contract: one scope serves exactly one write request at one write
//! timestamp. It is a duplicate filter for that request, not a transaction
//! isolation mechanism. Multi-statement atomicity stays with timestamp
//! ordering plus the outer write-ahead log; reusing a scope across
//! timestamps or across independent requests is a misuse and is rejected.
//!
//! Lifecycle: created at the write entry together with the write timestamp
//! (single and batch inserts each create one), threaded by mutable borrow
//! through context, vertex operation, sharded table and single table layers,
//! then destroyed exactly once by commit (merge), rollback (discard plus
//! logical delete of already applied rows through the existing undo path),
//! or process crash (memory dropped, never persisted).
//!
//! Visibility: scoped reads check the caller's buffer first and return that
//! binding; misses fall back to the existing global plus timestamp check.
//! Outside reads never see the buffer, so [`PkLookup`] keeps its two states
//! and cursor layers are unchanged. Cross-scope duplicate conflicts reuse the
//! existing read-lock probe plus write-lock recheck, still allocating once.
//!
//! Capacity: every scope buffers at most [`MAX_WRITE_SCOPE_KEYS`] keys;
//! staging beyond that fails the write so callers split batches instead of
//! growing without bound. Crash semantics: the buffer is memory only and
//! never reaches the baseline plus incremental persistence path.

use std::collections::HashMap;

use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult};

use super::id_indexer::IdKey;

/// Upper bound for staged primary keys in one write scope.
///
/// Referenced by the production insert path (`ShardedVertexTable` scoped
/// writes fail with `capacity_exceeded` past this count) so batches are split
/// instead of growing without bound.
pub const MAX_WRITE_SCOPE_KEYS: usize = 4096;

/// One staged primary-key binding owned by the caller.
#[derive(Debug, Clone)]
pub struct StagedPkBinding {
    /// Owning vertex label table.
    pub label: LabelId,
    /// Global internal id allocated by the immediate apply.
    pub global_id: u32,
}

/// Caller-owned buffer of uncommitted primary keys for one write timestamp.
///
/// All state lives in the caller and is passed by mutable borrow; shards and
/// indexes never retain it across calls.
#[derive(Debug, Default)]
pub struct WriteScope {
    write_ts: Timestamp,
    staged: HashMap<(LabelId, IdKey), StagedPkBinding>,
}

impl WriteScope {
    /// Create a scope for `write_ts`. Called at the write entry together with
    /// timestamp allocation; single and batch inserts each own one scope.
    pub fn new(write_ts: Timestamp) -> Self {
        Self {
            write_ts,
            staged: HashMap::new(),
        }
    }

    /// Write timestamp this scope was created for.
    pub fn write_ts(&self) -> Timestamp {
        self.write_ts
    }

    /// Reject reuse across write timestamps.
    ///
    /// One scope serves one request at one timestamp; callers must create a
    /// fresh scope per request instead of sharing it across statements.
    pub fn ensure_same_write_ts(&self, ts: Timestamp) -> StorageResult<()> {
        if self.write_ts != ts {
            return Err(StorageError::invalid_operation(format!(
                "write scope created for ts={} cannot serve ts={}: \
                 scopes are single-request dedup filters, not transaction write sets",
                self.write_ts, ts
            )));
        }
        Ok(())
    }

    /// Number of staged keys.
    pub fn len(&self) -> usize {
        self.staged.len()
    }

    /// Whether no keys are staged.
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    /// Whether `(label, key)` was staged by this scope.
    pub fn contains(&self, label: LabelId, key: &IdKey) -> bool {
        self.staged.contains_key(&(label, key.clone()))
    }

    /// Scoped binding for `(label, key)`, if staged by this scope.
    pub fn lookup(&self, label: LabelId, key: &IdKey) -> Option<&StagedPkBinding> {
        self.staged.get(&(label, key.clone()))
    }

    /// Record a successfully applied `(label, key)` binding.
    ///
    /// Fails when the same scope already staged the key (same-scope
    /// duplicate) or when the scope is full ([`MAX_WRITE_SCOPE_KEYS`]).
    pub fn record(&mut self, label: LabelId, key: IdKey, global_id: u32) -> StorageResult<()> {
        if self.staged.contains_key(&(label, key.clone())) {
            return Err(StorageError::vertex_already_exists(format!(
                "duplicate key in write scope: {:?}",
                key
            )));
        }
        if self.staged.len() >= MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        self.staged
            .insert((label, key), StagedPkBinding { label, global_id });
        Ok(())
    }

    /// Staged bindings for one label table, in arbitrary order.
    pub fn bindings_for_label(&self, label: LabelId) -> Vec<StagedPkBinding> {
        self.staged
            .values()
            .filter(|binding| binding.label == label)
            .cloned()
            .collect()
    }

    /// Global ids staged for one label table.
    pub fn global_ids_for_label(&self, label: LabelId) -> Vec<u32> {
        self.bindings_for_label(label)
            .into_iter()
            .map(|binding| binding.global_id)
            .collect()
    }

    /// Commit hook: global rows were applied at write time with timestamp
    /// ordering, so committing publishes visibility through the existing
    /// timestamp commit and WAL paths. The scope only drops its ownership
    /// records for `label` and reports how many bindings it owned.
    pub fn commit_label(&mut self, label: LabelId) -> usize {
        let before = self.staged.len();
        self.staged
            .retain(|(entry_label, _), _| *entry_label != label);
        before - self.staged.len()
    }

    /// Rollback hook: discard this label's staged ownership records. Already
    /// applied rows are removed by the caller through the existing undo path
    /// (same-timestamp logical delete), never here.
    pub fn rollback_label(&mut self, label: LabelId) {
        self.staged
            .retain(|(entry_label, _), _| *entry_label != label);
    }

    /// Discard every staged binding without touching global state.
    pub fn clear(&mut self) {
        self.staged.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_records_and_enforces_same_scope_duplicates() {
        let mut scope = WriteScope::new(10);
        scope
            .record(1, IdKey::Text("a".to_string()), 100)
            .expect("first stage succeeds");
        assert!(scope.contains(1, &IdKey::Text("a".to_string())));
        assert!(!scope.contains(2, &IdKey::Text("a".to_string())));
        let duplicate = scope.record(1, IdKey::Text("a".to_string()), 101);
        assert!(duplicate.is_err());
        assert_eq!(scope.len(), 1);
        assert_eq!(
            scope
                .lookup(1, &IdKey::Text("a".to_string()))
                .expect("staged")
                .global_id,
            100
        );
    }

    #[test]
    fn scope_rejects_writes_past_capacity() {
        let mut scope = WriteScope::new(10);
        for index in 0..MAX_WRITE_SCOPE_KEYS {
            scope
                .record(1, IdKey::Int(index as i64), index as u32)
                .expect("within capacity");
        }
        assert!(scope
            .record(1, IdKey::Int(MAX_WRITE_SCOPE_KEYS as i64), 0)
            .is_err());
    }

    #[test]
    fn scope_commit_and_rollback_drop_only_their_label() {
        let mut scope = WriteScope::new(10);
        scope.record(1, IdKey::Int(1), 11).unwrap();
        scope.record(2, IdKey::Int(1), 21).unwrap();
        assert_eq!(scope.commit_label(1), 1);
        assert!(!scope.contains(1, &IdKey::Int(1)));
        assert!(scope.contains(2, &IdKey::Int(1)));
        scope.rollback_label(2);
        assert!(scope.is_empty());
    }

    #[test]
    fn scope_rejects_cross_timestamp_reuse() {
        let scope = WriteScope::new(10);
        assert!(scope.ensure_same_write_ts(10).is_ok());
        assert!(scope.ensure_same_write_ts(11).is_err());
    }
}

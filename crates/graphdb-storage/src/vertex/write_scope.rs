//! Caller-owned transaction-local write staging.
//!
//! Online vertex writes never touch global table state directly. The scope
//! object carries the request's staged rows through context, vertex
//! operation and shard layers; the commit hook applies them to the tables
//! and the rollback hook drops them without any global write. Tables only
//! see staged rows at apply time.
//!
//! Contract: one scope serves exactly one write request at one write
//! timestamp. Reusing a scope across timestamps or across independent
//! requests is a misuse and is rejected.
//!
//! Lifecycle: created at the write entry together with the write timestamp,
//! threaded by mutable borrow through context, vertex operation and sharded
//! table layers, then destroyed exactly once by commit (apply plus drop) or
//! rollback (discard) or process crash (memory dropped, never persisted).
//! Single-row entries stage and apply inside their own call; batch entries
//! stage every row first and apply once at the commit hook, so a failed
//! batch leaves no partial application behind.
//!
//! Visibility: staged inserts are deduplicated by key in the scope, so
//! same-request duplicates fail before touching global state. Point reads
//! inside the staging request resolve staged keys through the scope-aware
//! lookup helpers; global reads keep their existing timestamp predicate and
//! never observe staged rows.
//!
//! Capacity: every scope stages at most [`MAX_WRITE_SCOPE_KEYS`] rows in
//! total; staging beyond that fails the write so callers split batches
//! instead of growing without bound. Crash semantics: the staging area is
//! memory only and never reaches the baseline plus incremental persistence
//! path. Offline barriered tools bypass scopes and write directly.

use std::collections::{HashMap, HashSet};

use graphdb_core::types::{LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

use super::id_indexer::IdKey;

/// Upper bound for staged rows in one write scope.
///
/// Referenced by the production write path (staged writes fail with
/// `capacity_exceeded` past this count) so batches are split instead of
/// growing without bound.
pub const MAX_WRITE_SCOPE_KEYS: usize = 4096;

/// One staged primary-key binding owned by the caller.
#[derive(Debug, Clone)]
pub struct StagedPkBinding {
    /// Owning vertex label table.
    pub label: LabelId,
    /// Global internal id once the commit hook applied the row, unset while
    /// the row is only staged.
    pub global_id: Option<u32>,
}

/// Caller-owned staging area for one write timestamp.
///
/// All state lives in the caller and is passed by mutable borrow; shards and
/// indexes never retain it across calls.
#[derive(Debug, Default)]
pub struct WriteScope {
    write_ts: Timestamp,
    staged: HashMap<(LabelId, IdKey), StagedPkBinding>,
    inserts: HashMap<(LabelId, IdKey), Vec<(String, Value)>>,
    updates: HashMap<(LabelId, u32), Vec<(String, Value)>>,
    deletes: HashSet<(LabelId, u32)>,
}

impl WriteScope {
    /// Create a scope for `write_ts`. Called at the write entry together with
    /// timestamp allocation; single and batch writes each own one scope.
    pub fn new(write_ts: Timestamp) -> Self {
        Self {
            write_ts,
            staged: HashMap::new(),
            inserts: HashMap::new(),
            updates: HashMap::new(),
            deletes: HashSet::new(),
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
                 scopes are single-request staging areas, not shared buffers",
                self.write_ts, ts
            )));
        }
        Ok(())
    }

    /// Number of staged rows (inserts plus updates plus deletes).
    pub fn len(&self) -> usize {
        self.staged.len() + self.updates.len() + self.deletes.len()
    }

    /// Whether nothing is staged.
    pub fn is_empty(&self) -> bool {
        self.staged.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }

    /// Whether `(label, key)` was staged by this scope.
    pub fn contains(&self, label: LabelId, key: &IdKey) -> bool {
        self.staged.contains_key(&(label, key.clone()))
    }

    /// Scoped binding for `(label, key)`, if staged by this scope.
    pub fn lookup(&self, label: LabelId, key: &IdKey) -> Option<&StagedPkBinding> {
        self.staged.get(&(label, key.clone()))
    }

    fn ensure_capacity(&self) -> StorageResult<()> {
        if self.len() >= MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        Ok(())
    }

    /// Stage one validated insert row.
    ///
    /// `properties` must already be validated and normalized (key shape,
    /// column types, primary-key mirror); the commit hook applies them
    /// without revalidation. Fails on same-scope duplicates and past
    /// capacity without touching global state.
    pub fn stage_insert(
        &mut self,
        label: LabelId,
        key: IdKey,
        properties: Vec<(String, Value)>,
    ) -> StorageResult<()> {
        if self.staged.contains_key(&(label, key.clone())) {
            return Err(StorageError::vertex_already_exists(format!(
                "duplicate key in write scope: {:?}",
                key
            )));
        }
        self.ensure_capacity()?;
        self.staged
            .insert((label, key.clone()), StagedPkBinding { label, global_id: None });
        self.inserts.insert((label, key), properties);
        Ok(())
    }

    /// Stage one property update against an already resolved global id.
    ///
    /// `properties` holds `(column, converted value)` pairs. The commit hook
    /// applies them in stage order after the staged inserts.
    pub fn stage_update(
        &mut self,
        label: LabelId,
        global_id: u32,
        properties: Vec<(String, Value)>,
    ) -> StorageResult<()> {
        self.ensure_capacity()?;
        self.updates
            .entry((label, global_id))
            .or_default()
            .extend(properties);
        Ok(())
    }

    /// Stage one delete against an already resolved global id. The commit
    /// hook applies staged deletes after inserts and updates.
    pub fn stage_delete(&mut self, label: LabelId, global_id: u32) -> StorageResult<()> {
        self.ensure_capacity()?;
        self.deletes.insert((label, global_id));
        Ok(())
    }

    /// Take every staged insert of one label for the commit apply, in
    /// arbitrary order. The caller applies them and records the allocated
    /// ids back through [`Self::bind_applied`].
    pub fn take_inserts_for_label(&mut self, label: LabelId) -> Vec<(IdKey, Vec<(String, Value)>)> {
        let keys: Vec<(LabelId, IdKey)> = self
            .inserts
            .keys()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for map_key in keys {
            if let Some(props) = self.inserts.remove(&map_key) {
                out.push((map_key.1, props));
            }
        }
        out
    }

    /// Take every staged update of one label for the commit apply.
    pub fn take_updates_for_label(
        &mut self,
        label: LabelId,
    ) -> Vec<(u32, Vec<(String, Value)>)> {
        let keys: Vec<(LabelId, u32)> = self
            .updates
            .keys()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for map_key in keys {
            if let Some(props) = self.updates.remove(&map_key) {
                out.push((map_key.1, props));
            }
        }
        out
    }

    /// Take every staged delete of one label for the commit apply.
    pub fn take_deletes_for_label(&mut self, label: LabelId) -> Vec<u32> {
        let keys: Vec<(LabelId, u32)> = self
            .deletes
            .iter()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for map_key in &keys {
            if self.deletes.remove(map_key) {
                out.push(map_key.1);
            }
        }
        out
    }

    /// Record the global id the commit hook allocated for a staged key, so
    /// same-request lookups after the apply resolve without a global probe.
    pub fn bind_applied(&mut self, label: LabelId, key: &IdKey, global_id: u32) {
        if let Some(binding) = self.staged.get_mut(&(label, key.clone())) {
            binding.global_id = Some(global_id);
        }
    }

    /// Staged bindings for one label table, in arbitrary order.
    pub fn bindings_for_label(&self, label: LabelId) -> Vec<StagedPkBinding> {
        self.staged
            .values()
            .filter(|binding| binding.label == label)
            .cloned()
            .collect()
    }

    /// Applied global ids staged for one label table.
    pub fn global_ids_for_label(&self, label: LabelId) -> Vec<u32> {
        self.bindings_for_label(label)
            .into_iter()
            .filter_map(|binding| binding.global_id)
            .collect()
    }

    /// Commit hook: drop this label's staged ownership records and report
    /// how many bindings it owned. Row application already happened in the
    /// table commit apply ahead of this call.
    pub fn commit_label(&mut self, label: LabelId) -> usize {
        let before = self.staged.len();
        self.staged
            .retain(|(entry_label, _), _| *entry_label != label);
        self.inserts
            .retain(|(entry_label, _), _| *entry_label != label);
        self.updates
            .retain(|(entry_label, _), _| *entry_label != label);
        self.deletes.retain(|(entry_label, _)| *entry_label != label);
        before - self.staged.len()
    }

    /// Rollback hook: discard this label's staged rows. Applied rows never
    /// exist at this point for staged entries; already applied rows from a
    /// committed apply are removed by the caller through the table undo
    /// path, never here.
    pub fn rollback_label(&mut self, label: LabelId) {
        self.staged
            .retain(|(entry_label, _), _| *entry_label != label);
        self.inserts
            .retain(|(entry_label, _), _| *entry_label != label);
        self.updates
            .retain(|(entry_label, _), _| *entry_label != label);
        self.deletes.retain(|(entry_label, _)| *entry_label != label);
    }

    /// Record a successfully applied `(label, key)` binding.
    ///
    /// Staging entry point kept for direct-apply callers that allocate the
    /// global id synchronously: fails on same-scope duplicates and past
    /// capacity.
    pub fn record(&mut self, label: LabelId, key: IdKey, global_id: u32) -> StorageResult<()> {
        if self.staged.contains_key(&(label, key.clone())) {
            return Err(StorageError::vertex_already_exists(format!(
                "duplicate key in write scope: {:?}",
                key
            )));
        }
        self.ensure_capacity()?;
        self.staged.insert(
            (label, key),
            StagedPkBinding {
                label,
                global_id: Some(global_id),
            },
        );
        Ok(())
    }

    /// Discard every staged row without touching global state.
    pub fn clear(&mut self) {
        self.staged.clear();
        self.inserts.clear();
        self.updates.clear();
        self.deletes.clear();
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
            Some(100)
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

    #[test]
    fn scope_stages_rows_and_hands_them_to_commit() {
        use graphdb_core::Value;
        let mut scope = WriteScope::new(10);
        scope
            .stage_insert(
                1,
                IdKey::Text("a".to_string()),
                vec![("name".to_string(), Value::from("a"))],
            )
            .unwrap();
        assert!(scope.contains(1, &IdKey::Text("a".to_string())));
        assert!(scope
            .stage_insert(1, IdKey::Text("a".to_string()), Vec::new())
            .is_err());
        scope.stage_update(1, 7, vec![("age".to_string(), Value::from(3))]).unwrap();
        scope.stage_delete(1, 9).unwrap();
        assert_eq!(scope.len(), 3);

        let inserts = scope.take_inserts_for_label(1);
        assert_eq!(inserts.len(), 1);
        scope.bind_applied(1, &IdKey::Text("a".to_string()), 42);
        assert_eq!(
            scope
                .lookup(1, &IdKey::Text("a".to_string()))
                .expect("staged")
                .global_id,
            Some(42)
        );
        assert_eq!(scope.take_updates_for_label(1).len(), 1);
        assert_eq!(scope.take_deletes_for_label(1), vec![9]);
        assert_eq!(scope.commit_label(1), 1);
        assert!(scope.is_empty());
    }

    #[test]
    fn scope_rollback_discards_every_kind() {
        use graphdb_core::Value;
        let mut scope = WriteScope::new(10);
        scope
            .stage_insert(1, IdKey::Int(1), vec![("v".to_string(), Value::from(1))])
            .unwrap();
        scope.stage_update(1, 2, vec![("v".to_string(), Value::from(2))]).unwrap();
        scope.stage_delete(2, 3).unwrap();
        scope.rollback_label(1);
        assert!(!scope.contains(1, &IdKey::Int(1)));
        assert!(!scope.is_empty());
        scope.rollback_label(2);
        assert!(scope.is_empty());
    }
}

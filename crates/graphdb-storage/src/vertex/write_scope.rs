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
//! Online entries are absorbed into transaction staging at statement end and
//! applied at the transaction commit point; offline barriered tools apply
//! directly inside their own call. Batch entries stage every row first and
//! apply once at the commit hook, so a failed batch leaves no partial
//! application behind.
//!
//! Visibility: staged inserts are deduplicated by key in the scope, so
//! same-request duplicates fail before touching global state. Global reads
//! keep their existing timestamp predicate and never observe staged rows.
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

/// Caller-owned staging area for one write timestamp.
///
/// All state lives in the caller and is passed by mutable borrow; shards and
/// indexes never retain it across calls.
#[derive(Debug, Default)]
pub struct WriteScope {
    write_ts: Timestamp,
    inserts: HashMap<(LabelId, IdKey), (u32, Vec<(String, Value)>)>,
    updates: HashMap<(LabelId, u32), Vec<(String, Value)>>,
    deletes: HashSet<(LabelId, u32)>,
}

impl WriteScope {
    /// Create a scope for `write_ts`. Called at the write entry together with
    /// timestamp allocation; single and batch writes each own one scope.
    pub fn new(write_ts: Timestamp) -> Self {
        Self {
            write_ts,
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

    /// Rebuild a scope from transaction-level staging rows for the commit
    /// apply. Bypasses the per-scope capacity bound: those rows already
    /// passed the transaction-level bound when they were staged.
    pub(crate) fn from_staged(
        write_ts: Timestamp,
        inserts: HashMap<(LabelId, IdKey), (u32, Vec<(String, Value)>)>,
        updates: HashMap<(LabelId, u32), Vec<(String, Value)>>,
        deletes: HashSet<(LabelId, u32)>,
    ) -> Self {
        Self {
            write_ts,
            inserts,
            updates,
            deletes,
        }
    }

    /// Number of staged rows (inserts plus updates plus deletes).
    pub fn len(&self) -> usize {
        self.inserts.len() + self.updates.len() + self.deletes.len()
    }

    /// Whether nothing is staged.
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.updates.is_empty() && self.deletes.is_empty()
    }

    fn ensure_capacity(&self) -> StorageResult<()> {
        if self.len() >= MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        Ok(())
    }

    /// Stage one validated insert row against a reserved global vertex id.
    ///
    /// `properties` must already be validated and normalized (key shape,
    /// column types, primary-key mirror); the commit hook applies them
    /// without revalidation. `reserved_global_id` was obtained from the
    /// owning table's reserve entry for `key`, so the apply binds the key
    /// to exactly that id. Fails on same-scope duplicates and past capacity
    /// without touching global state; the caller releases the reservation
    /// on failure.
    pub fn stage_insert(
        &mut self,
        label: LabelId,
        key: IdKey,
        reserved_global_id: u32,
        properties: Vec<(String, Value)>,
    ) -> StorageResult<()> {
        if self.inserts.contains_key(&(label, key.clone())) {
            return Err(StorageError::vertex_already_exists(format!(
                "duplicate key in write scope: {:?}",
                key
            )));
        }
        self.ensure_capacity()?;
        self.inserts
            .insert((label, key), (reserved_global_id, properties));
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
    /// arbitrary order, each with its reserved global id. The caller
    /// applies them, reports the staged key to allocated global id mapping
    /// back to the commit caller, and releases the reservations of rows it
    /// ultimately does not apply.
    pub fn take_inserts_for_label(
        &mut self,
        label: LabelId,
    ) -> Vec<(IdKey, u32, Vec<(String, Value)>)> {
        let keys: Vec<(LabelId, IdKey)> = self
            .inserts
            .keys()
            .filter(|(entry_label, _)| *entry_label == label)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(keys.len());
        for map_key in keys {
            if let Some((reserved, props)) = self.inserts.remove(&map_key) {
                out.push((map_key.1, reserved, props));
            }
        }
        out
    }

    /// Take every staged update of one label for the commit apply.
    pub fn take_updates_for_label(&mut self, label: LabelId) -> Vec<(u32, Vec<(String, Value)>)> {
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

    /// Labels with staged rows in any of the three sets, sorted.
    pub fn labels(&self) -> Vec<LabelId> {
        let mut labels: Vec<LabelId> = self
            .inserts
            .keys()
            .map(|(label, _)| *label)
            .chain(self.updates.keys().map(|(label, _)| *label))
            .chain(self.deletes.iter().map(|(label, _)| *label))
            .collect();
        labels.sort_unstable();
        labels.dedup();
        labels
    }

    /// Commit hook: drop this label's staged rows. Row application already
    /// happened in the table commit apply ahead of this call.
    pub fn commit_label(&mut self, label: LabelId) {
        self.inserts
            .retain(|(entry_label, _), _| *entry_label != label);
        self.updates
            .retain(|(entry_label, _), _| *entry_label != label);
        self.deletes
            .retain(|(entry_label, _)| *entry_label != label);
    }

    /// Rollback hook: discard this label's staged rows and return the
    /// reserved global ids of the dropped inserts so the caller can release
    /// them. Applied rows never exist at this point for staged entries;
    /// already applied rows from a committed apply are removed by the
    /// caller through the table undo path, never here.
    pub fn rollback_label(&mut self, label: LabelId) -> Vec<u32> {
        let dropped: Vec<u32> = self
            .inserts
            .iter()
            .filter(|((entry_label, _), _)| *entry_label == label)
            .map(|(_, (reserved, _))| *reserved)
            .collect();
        self.inserts
            .retain(|(entry_label, _), _| *entry_label != label);
        self.updates
            .retain(|(entry_label, _), _| *entry_label != label);
        self.deletes
            .retain(|(entry_label, _)| *entry_label != label);
        dropped
    }

    /// Discard every staged row without touching global state.
    pub fn clear(&mut self) {
        self.inserts.clear();
        self.updates.clear();
        self.deletes.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

    fn props(name: &str) -> Vec<(String, Value)> {
        vec![("name".to_string(), Value::from(name))]
    }

    #[test]
    fn scope_rejects_same_scope_duplicates() {
        let mut scope = WriteScope::new(10);
        scope
            .stage_insert(1, IdKey::Text("a".to_string()), 100, props("a"))
            .expect("first stage succeeds");
        let duplicate = scope.stage_insert(1, IdKey::Text("a".to_string()), 101, props("a"));
        assert!(duplicate.is_err());
        assert_eq!(scope.len(), 1);
    }

    #[test]
    fn scope_rejects_writes_past_capacity() {
        let mut scope = WriteScope::new(10);
        for index in 0..MAX_WRITE_SCOPE_KEYS {
            scope
                .stage_insert(1, IdKey::Int(index as i64), index as u32, Vec::new())
                .expect("within capacity");
        }
        assert!(scope
            .stage_insert(
                1,
                IdKey::Int(MAX_WRITE_SCOPE_KEYS as i64),
                MAX_WRITE_SCOPE_KEYS as u32,
                Vec::new()
            )
            .is_err());
    }

    #[test]
    fn scope_commit_and_rollback_drop_only_their_label() {
        let mut scope = WriteScope::new(10);
        scope
            .stage_insert(1, IdKey::Int(1), 10, props("a"))
            .unwrap();
        scope
            .stage_insert(2, IdKey::Int(1), 20, props("b"))
            .unwrap();
        scope.commit_label(1);
        assert_eq!(scope.len(), 1);
        let released = scope.rollback_label(2);
        assert_eq!(released, vec![20]);
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
        let mut scope = WriteScope::new(10);
        scope
            .stage_insert(1, IdKey::Text("a".to_string()), 7, props("a"))
            .unwrap();
        scope
            .stage_update(1, 7, vec![("age".to_string(), Value::from(3))])
            .unwrap();
        scope.stage_delete(1, 9).unwrap();
        assert_eq!(scope.len(), 3);

        let inserts = scope.take_inserts_for_label(1);
        assert_eq!(inserts.len(), 1);
        assert_eq!(inserts[0].1, 7);
        assert_eq!(scope.take_updates_for_label(1).len(), 1);
        assert_eq!(scope.take_deletes_for_label(1), vec![9]);
        scope.commit_label(1);
        assert!(scope.is_empty());
    }

    #[test]
    fn scope_rollback_discards_every_kind() {
        let mut scope = WriteScope::new(10);
        scope.stage_insert(1, IdKey::Int(1), 5, props("a")).unwrap();
        scope
            .stage_update(1, 2, vec![("v".to_string(), Value::from(2))])
            .unwrap();
        scope.stage_delete(2, 3).unwrap();
        let released = scope.rollback_label(1);
        assert_eq!(released, vec![5]);
        assert!(!scope.is_empty());
        scope.rollback_label(2);
        assert!(scope.is_empty());
    }
}

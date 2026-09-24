use super::ShardedVertexTable;
use crate::vertex::{IdKey, PkLookup, WriteScope, MAX_WRITE_SCOPE_KEYS};
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};

impl ShardedVertexTable {
    pub fn insert(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_str(external_id);
        // Compare-and-swap fast path: a read-lock dedup hit returns the
        // duplicate error without an exclusive section. Misses (and
        // timestamp-deleted keys) fall through to the write-locked atomic
        // path, which rechecks before allocating, so concurrent same-key
        // inserts still allocate exactly once.
        let key = IdKey::Text(external_id.to_string());
        if self.shards[idx].read().is_duplicate(&key, ts) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        let mut table = self.shards[idx].write();
        let local_id = table.insert(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn insert_by_i64(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<u32> {
        let idx = self.shard_index_by_i64(external_id);
        let key = IdKey::Int(external_id);
        if self.shards[idx].read().is_duplicate(&key, ts) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        let mut table = self.shards[idx].write();
        let local_id = table.insert_by_i64(external_id, properties, ts)?;
        Ok(self.record_allocation(idx, local_id))
    }

    pub fn delete(&self, external_id: &str, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].write();
        table.delete(external_id, ts)
    }

    pub fn delete_by_i64(&self, external_id: i64, ts: Timestamp) -> StorageResult<()> {
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].write();
        table.delete_by_i64(external_id, ts)
    }

    pub fn update_property(
        &self,
        global_id: u32,
        col_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.update_property(local_id, col_name, value, ts)
    }

    pub fn delete_by_internal_id(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.delete_by_internal_id(local_id, ts)
    }

    pub fn revert_delete(&self, global_id: u32, ts: Timestamp) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.revert_delete(local_id, ts)
    }

    pub fn update_property_by_id(
        &self,
        global_id: u32,
        col_id: i32,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (idx, local_id) = self.decode_id(global_id);
        let mut table = self.shards[idx].write();
        table.update_property_by_id(local_id, col_id, value, ts)
    }

    pub fn batch_delete(&self, external_ids: &[&str], ts: Timestamp) -> StorageResult<usize> {
        // Route ids to their owning shard and delete each shard's batch under
        // a single lock, instead of locking per id.
        let mut by_shard: Vec<Vec<&str>> = vec![Vec::new(); self.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_str(id)].push(id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].write().batch_delete(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn batch_delete_i64(&self, external_ids: &[i64], ts: Timestamp) -> StorageResult<usize> {
        let mut by_shard: Vec<Vec<i64>> = vec![Vec::new(); self.num_shards];
        for id in external_ids {
            by_shard[self.shard_index_by_i64(*id)].push(*id);
        }
        let mut total = 0;
        for (idx, ids) in by_shard.iter().enumerate() {
            if !ids.is_empty() {
                total += self.shards[idx].write().batch_delete_i64(ids, ts)?;
            }
        }
        Ok(total)
    }

    pub fn reserve_id_capacity(&self, additional: usize) {
        for shard in &self.shards {
            shard.write().reserve_id_capacity(additional);
        }
    }

    /// Shard-grouped batch apply of a string-keyed insert batch.
    ///
    /// Rows are first routed to their owning shards without taking any lock
    /// (the routing step), then each non-empty shard group is applied under
    /// a single write-lock hold in input order (the merge step). Results are
    /// aligned with the input: every row is attempted even after earlier
    /// failures, so the caller rolls back every `Ok` entry when any `Err`
    /// is present. Duplicate keys fail per row exactly as [`insert`] does.
    ///
    /// [`insert`]: Self::insert
    pub fn insert_batch_str(
        &self,
        rows: &[(&str, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.num_shards];
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_str(external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let mut table = self.shards[shard_idx].write();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert(external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }

    /// Shard-grouped staged apply of an integer-keyed insert batch. Same
    /// staging/merge contract as [`insert_batch_str`].
    ///
    /// [`insert_batch_str`]: Self::insert_batch_str
    pub fn insert_batch_i64(
        &self,
        rows: &[(i64, &[(String, Value)])],
        ts: Timestamp,
    ) -> Vec<StorageResult<u32>> {
        let mut by_shard: Vec<Vec<usize>> = vec![Vec::new(); self.num_shards];
        for (pos, (external_id, _)) in rows.iter().enumerate() {
            by_shard[self.shard_index_by_i64(*external_id)].push(pos);
        }
        let mut out: Vec<Option<StorageResult<u32>>> = (0..rows.len()).map(|_| None).collect();
        for (shard_idx, positions) in by_shard.iter().enumerate() {
            if positions.is_empty() {
                continue;
            }
            let mut table = self.shards[shard_idx].write();
            for &pos in positions {
                let (external_id, properties) = &rows[pos];
                let result = table
                    .insert_by_i64(*external_id, properties, ts)
                    .map(|local_id| self.record_allocation(shard_idx, local_id));
                out[pos] = Some(result);
            }
        }
        out.into_iter()
            .map(|slot| slot.expect("every input row is routed once"))
            .collect()
    }

    /// Scoped insert threading the caller-owned write scope.
    ///
    /// Same-scope duplicates fail before touching global state; cross-scope
    /// conflicts reuse the read-lock probe plus write-lock recheck in the
    /// single-table path, still allocating once. On success the binding is
    /// recorded in the caller's scope (capacity [`MAX_WRITE_SCOPE_KEYS`]).
    /// The shard never retains the scope across calls.
    pub fn insert_with_scope(
        &self,
        external_id: &str,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<u32> {
        let key = IdKey::Text(external_id.to_string());
        if scope.contains(self.label, &key) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        if scope.len() >= MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        let idx = self.shard_index_by_str(external_id);
        let mut table = self.shards[idx].write();
        let local_id = table.insert_with_scope(key.clone(), properties, ts, scope)?;
        let global_id = self.record_allocation(idx, local_id);
        scope.record(self.label, key, global_id)?;
        Ok(global_id)
    }

    /// Integer-keyed scoped insert. Same contract as [`Self::insert_with_scope`].
    ///
    /// [`Self::insert_with_scope`]: Self::insert_with_scope
    pub fn insert_by_i64_with_scope(
        &self,
        external_id: i64,
        properties: &[(String, Value)],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> StorageResult<u32> {
        let key = IdKey::Int(external_id);
        if scope.contains(self.label, &key) {
            return Err(StorageError::vertex_already_exists(format!("{:?}", key)));
        }
        if scope.len() >= MAX_WRITE_SCOPE_KEYS {
            return Err(StorageError::capacity_exceeded());
        }
        let idx = self.shard_index_by_i64(external_id);
        let mut table = self.shards[idx].write();
        let local_id = table.insert_with_scope(key.clone(), properties, ts, scope)?;
        let global_id = self.record_allocation(idx, local_id);
        scope.record(self.label, key, global_id)?;
        Ok(global_id)
    }

    /// Scoped batch insert: every row is staged through the caller's scope.
    /// Same-scope duplicates fail per row; global conflicts follow the plain
    /// batch contract. Successful rows are recorded in scope order.
    pub fn insert_batch_str_with_scope(
        &self,
        rows: &[(&str, &[(String, Value)])],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<u32>> {
        let results = self.insert_batch_str(rows, ts);
        for ((external_id, _), result) in rows.iter().zip(results.iter()) {
            if let Ok(global_id) = result {
                let key = IdKey::Text(external_id.to_string());
                if scope.contains(self.label, &key) {
                    continue;
                }
                if scope.len() >= MAX_WRITE_SCOPE_KEYS {
                    break;
                }
                let _ = scope.record(self.label, key, *global_id);
            }
        }
        results
    }

    /// Integer-keyed scoped batch insert. Same contract as
    /// [`Self::insert_batch_str_with_scope`].
    ///
    /// [`Self::insert_batch_str_with_scope`]: Self::insert_batch_str_with_scope
    pub fn insert_batch_i64_with_scope(
        &self,
        rows: &[(i64, &[(String, Value)])],
        ts: Timestamp,
        scope: &mut WriteScope,
    ) -> Vec<StorageResult<u32>> {
        let results = self.insert_batch_i64(rows, ts);
        for ((external_id, _), result) in rows.iter().zip(results.iter()) {
            if let Ok(global_id) = result {
                let key = IdKey::Int(*external_id);
                if scope.contains(self.label, &key) {
                    continue;
                }
                if scope.len() >= MAX_WRITE_SCOPE_KEYS {
                    break;
                }
                let _ = scope.record(self.label, key, *global_id);
            }
        }
        results
    }

    /// Commit hook for one label table: global rows were applied at write
    /// time, so committing drops this table's ownership records from the
    /// caller's scope and reports the merged count. Called before the
    /// timestamp commit; the WAL commit entries are the durability point.
    pub fn commit_write_scope(&self, scope: &mut WriteScope) -> usize {
        scope.commit_label(self.label)
    }

    /// Rollback hook for one label table: discard this table's staged
    /// ownership records, then logically delete the already applied rows
    /// through the existing undo semantics (same-timestamp tombstone hides
    /// the row). Failures to delete are best-effort; the timestamp abort
    /// still hides the rows through the pending gate.
    pub fn rollback_write_scope(&self, scope: &mut WriteScope, ts: Timestamp) {
        let globals = scope.global_ids_for_label(self.label);
        scope.rollback_label(self.label);
        for global_id in globals {
            let _ = self.delete_by_internal_id(global_id, ts);
        }
    }

    /// Scoped primary-key lookup: the caller's buffer wins, otherwise the
    /// global committed area. Outside scopes never see the buffer, so the
    /// two-state [`PkLookup`] contract is unchanged.
    pub fn lookup_pk_with_scope(
        &self,
        external_id: &str,
        ts: Timestamp,
        scope: &WriteScope,
    ) -> PkLookup {
        let key = IdKey::Text(external_id.to_string());
        if let Some(binding) = scope.lookup(self.label, &key) {
            return PkLookup::Visible(binding.global_id);
        }
        let idx = self.shard_index_by_str(external_id);
        match self.shards[idx]
            .read()
            .lookup_internal_id_scoped(&key, ts, scope)
        {
            PkLookup::Visible(local_id) => PkLookup::Visible(self.encode_id(idx, local_id)),
            PkLookup::Missing => PkLookup::Missing,
        }
    }

    /// Integer-keyed scoped lookup. Same contract as
    /// [`Self::lookup_pk_with_scope`].
    ///
    /// [`Self::lookup_pk_with_scope`]: Self::lookup_pk_with_scope
    pub fn lookup_pk_by_i64_with_scope(
        &self,
        external_id: i64,
        ts: Timestamp,
        scope: &WriteScope,
    ) -> PkLookup {
        let key = IdKey::Int(external_id);
        if let Some(binding) = scope.lookup(self.label, &key) {
            return PkLookup::Visible(binding.global_id);
        }
        let idx = self.shard_index_by_i64(external_id);
        match self.shards[idx]
            .read()
            .lookup_internal_id_scoped(&key, ts, scope)
        {
            PkLookup::Visible(local_id) => PkLookup::Visible(self.encode_id(idx, local_id)),
            PkLookup::Missing => PkLookup::Missing,
        }
    }
}

#[cfg(test)]
mod scoped_tests {
    use super::*;
    use crate::types::StoragePropertyDef;
    use crate::vertex::VertexSchema;
    use graphdb_core::DataType;

    fn test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 7,
            label_name: "scoped".to_string(),
            properties: vec![StoragePropertyDef::new(
                "name".to_string(),
                DataType::String,
            )],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    fn props(name: &str) -> Vec<(String, Value)> {
        vec![("name".to_string(), Value::from(name))]
    }

    #[test]
    fn scoped_insert_is_self_consistent_and_older_snapshots_miss() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let write_ts: Timestamp = 100;
        let mut scope = WriteScope::new(write_ts);
        let global = table
            .insert_with_scope("k1", &props("k1"), write_ts, &mut scope)
            .unwrap();
        assert_eq!(
            table.lookup_pk_with_scope("k1", write_ts, &scope),
            PkLookup::Visible(global)
        );
        assert!(table.get_by_internal_id(global, write_ts).is_some());
        assert_eq!(table.lookup_pk("absent", write_ts), PkLookup::Missing);
        assert_eq!(
            table.lookup_pk("k1", write_ts - 1),
            PkLookup::Missing,
            "older snapshots miss uncommitted keys by timestamp ordering"
        );
        assert_eq!(table.commit_write_scope(&mut scope), 1);
        assert!(scope.is_empty());
        assert!(table.get_by_internal_id(global, write_ts).is_some());
    }

    #[test]
    fn same_scope_duplicate_fails_without_extra_allocation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        table
            .insert_with_scope("dup", &props("dup"), ts, &mut scope)
            .unwrap();
        assert!(table
            .insert_with_scope("dup", &props("dup"), ts, &mut scope)
            .is_err());
        assert_eq!(table.total_count(), 1);
    }

    #[test]
    fn cross_scope_conflict_still_allocates_once() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut first = WriteScope::new(ts);
        let mut second = WriteScope::new(ts);
        assert!(table
            .insert_with_scope("hot", &props("hot"), ts, &mut first)
            .is_ok());
        assert!(table
            .insert_with_scope("hot", &props("hot"), ts, &mut second)
            .is_err());
        assert_eq!(table.total_count(), 1);
        assert!(second.is_empty());
    }

    #[test]
    fn rollback_discards_scope_and_hides_applied_rows() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        let global = table
            .insert_with_scope("tmp", &props("tmp"), ts, &mut scope)
            .unwrap();
        table.rollback_write_scope(&mut scope, ts);
        assert!(scope.is_empty());
        assert!(table.get_by_internal_id(global, ts).is_none());
        assert_eq!(table.lookup_pk("tmp", ts), PkLookup::Missing);
    }

    #[test]
    fn over_limit_scoped_write_is_rejected_before_global_mutation() {
        let table = ShardedVertexTable::with_config(7, "scoped".to_string(), test_schema(), 4);
        let ts: Timestamp = 100;
        let mut scope = WriteScope::new(ts);
        for index in 0..MAX_WRITE_SCOPE_KEYS {
            scope
                .record(7, IdKey::Int(index as i64), index as u32)
                .expect("prefill within capacity");
        }
        assert!(table
            .insert_with_scope("overflow", &props("overflow"), ts, &mut scope)
            .is_err());
        assert_eq!(table.total_count(), 0);
    }

    #[test]
    fn concurrent_scoped_same_key_inserts_allocate_once() {
        use std::sync::Arc;
        let table = Arc::new(ShardedVertexTable::with_config(
            7,
            "scoped".to_string(),
            test_schema(),
            8,
        ));
        let ts: Timestamp = 100;
        let mut handles = Vec::new();
        for _ in 0..8 {
            let table = Arc::clone(&table);
            handles.push(std::thread::spawn(move || {
                let mut scope = WriteScope::new(ts);
                table
                    .insert_with_scope("race", &props("race"), ts, &mut scope)
                    .map(|_| ())
            }));
        }
        let mut oks = 0usize;
        for handle in handles {
            if handle.join().unwrap().is_ok() {
                oks += 1;
            }
        }
        assert_eq!(oks, 1);
        assert_eq!(table.total_count(), 1);
    }
}

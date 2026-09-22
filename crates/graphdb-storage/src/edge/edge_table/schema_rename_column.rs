//! Staged rename-column state machine for edge property columns.
//!
//! Renaming a column moves through explicit states so every step can roll
//! back: `prepare` validates and snapshots the rename without touching
//! storage, and `publish` records the write-ahead log entry first, then
//! renames the physical column, the schema entry and the name cache, and
//! finally records history (restoring everything from the snapshot when
//! history recording fails). `abort` drops the pending change before anything
//! is renamed. Pending state lives only in memory: a crash before publishing
//! is equivalent to an abort because reload rebuilds the property store from
//! the published schema.

use graphdb_core::{StorageError, StorageResult};

use super::core::EdgeStore;
use crate::schema::ChangeDetails;

/// Lifecycle state of one pending rename-column change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRenameColumnState {
    /// Validated and snapshotted, nothing renamed.
    Prepared,
}

/// One in-flight rename-column change.
///
/// The schema position and name-cache entry are snapshotted at prepare time
/// so a failed publish restores the exact previous names.
#[derive(Debug, Clone)]
pub struct PendingRenameColumn {
    pub old_name: String,
    pub new_name: String,
    pub schema_index: usize,
    pub cached_index: Option<usize>,
    pub state: PendingRenameColumnState,
}

impl EdgeStore {
    /// Borrow the pending rename-column change, if any.
    pub fn pending_rename_column(&self) -> Option<&PendingRenameColumn> {
        self.pending_rename_column.as_ref()
    }

    /// Validate a rename-column request and snapshot the affected names.
    ///
    /// Touches nothing: the physical column, the schema, the name cache and
    /// history stay unchanged until publishing. Renaming preserves column
    /// arity, so inline record forms need no rebuild hint here, unlike
    /// add/drop which change the property count.
    pub fn prepare_rename_property(&mut self, old_name: &str, new_name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.pending_add_column.is_some()
            || self.pending_drop_column.is_some()
            || self.pending_rename_column.is_some()
        {
            return Err(StorageError::invalid_operation(
                crate::edge::SCHEMA_CHANGE_PENDING_MSG.to_string(),
            ));
        }
        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }
        let schema_index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == old_name)
            .ok_or_else(|| StorageError::column_not_found(old_name.to_string()))?;
        if !self.properties.has_property(old_name) {
            return Err(StorageError::column_not_found(old_name.to_string()));
        }
        let cached_index = self.property_index_cache.get(old_name).copied();
        self.pending_rename_column = Some(PendingRenameColumn {
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
            schema_index,
            cached_index,
            state: PendingRenameColumnState::Prepared,
        });
        Ok(())
    }

    /// Execute the prepared rename: log, physical column, schema entry,
    /// name cache, history.
    ///
    /// The log entry is appended before any mutation so a crash after the
    /// log but before the checkpoint replays the rename. A history failure
    /// renames everything back, leaving the table as if publishing never ran.
    /// The pending slot is kept on failure so the caller can retry or abort.
    pub fn publish_pending_rename_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_rename_column.clone().ok_or_else(|| {
            StorageError::invalid_operation(
                "no pending rename-column change to publish".to_string(),
            )
        })?;
        if pending.state != PendingRenameColumnState::Prepared {
            return Err(StorageError::invalid_operation(
                "pending rename-column change is not prepared".to_string(),
            ));
        }
        if let Some(dir) = self.wal_dir.clone() {
            super::wal::append_ops(
                &dir,
                &[super::wal::EdgeWalOp::SchemaRename {
                    old_name: pending.old_name.clone(),
                    new_name: pending.new_name.clone(),
                }],
            )?;
        }
        self.properties
            .rename_property(&pending.old_name, &pending.new_name)?;
        self.schema.properties[pending.schema_index].name = pending.new_name.clone();
        if let Some(idx) = self.property_index_cache.remove(&pending.old_name) {
            self.property_index_cache
                .insert(pending.new_name.clone(), idx);
        }
        if let Err(error) = self.record_schema_change(ChangeDetails::PropertyRenamed {
            old_name: pending.old_name.clone(),
            new_name: pending.new_name.clone(),
        }) {
            let _ = self
                .properties
                .rename_property(&pending.new_name, &pending.old_name);
            self.schema.properties[pending.schema_index].name = pending.old_name.clone();
            if let Some(idx) = self.property_index_cache.remove(&pending.new_name) {
                self.property_index_cache
                    .insert(pending.old_name.clone(), idx);
            }
            return Err(error);
        }
        self.pending_rename_column = None;
        // Renames change every owner shard's schema: trace them all and
        // carry the rename into the per-group scopes.
        self.rename_property_column_in_dirt(&pending.old_name, &pending.new_name);
        self.trace_all_owner_groups_for_columns(std::slice::from_ref(&pending.new_name));
        Ok(())
    }

    /// Drop the pending change. Nothing was renamed yet, so there is nothing
    /// to restore.
    pub fn abort_pending_rename_property(&mut self) -> StorageResult<()> {
        self.pending_rename_column.take().ok_or_else(|| {
            StorageError::invalid_operation("no pending rename-column change to abort".to_string())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordForm};
    use crate::types::StoragePropertyDef;
    use graphdb_core::Value;

    fn make_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds")
    }

    #[test]
    fn staged_rename_full_lifecycle() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert should succeed");
        table
            .prepare_rename_property("weight", "mass")
            .expect("prepare should succeed");
        assert!(table.properties.has_property("weight"));
        table
            .publish_pending_rename_property()
            .expect("publish should succeed");
        assert!(table.pending_rename_column().is_none());
        assert!(!table.properties.has_property("weight"));
        assert!(table.properties.has_property("mass"));
        assert!(table.schema.properties.iter().any(|p| p.name == "mass"));
        assert!(table.has_edge(0, 1, 0, 200));
    }

    #[test]
    fn abort_after_prepare_leaves_no_trace() {
        let mut table = make_table();
        table
            .prepare_rename_property("weight", "mass")
            .expect("prepare should succeed");
        table
            .abort_pending_rename_property()
            .expect("abort should succeed");
        assert!(table.pending_rename_column().is_none());
        assert!(table.properties.has_property("weight"));
        assert!(!table.properties.has_property("mass"));
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut table = make_table();
        assert!(table.publish_pending_rename_property().is_err());
        assert!(table.abort_pending_rename_property().is_err());
        assert!(table.prepare_rename_property("missing", "mass").is_err());
        table
            .prepare_rename_property("weight", "mass")
            .expect("first prepare should succeed");
        assert!(table.prepare_rename_property("weight", "other").is_err());
        assert!(table
            .prepare_add_property("extra".to_string(), graphdb_core::DataType::Int, true, None)
            .is_err());
    }

    #[test]
    fn prepared_rename_does_not_survive_reload() {
        let mut table = make_table();
        table
            .prepare_rename_property("weight", "mass")
            .expect("prepare should succeed");
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(loaded.properties.has_property("weight"));
        assert!(!loaded.properties.has_property("mass"));
        assert!(loaded.pending_rename_column().is_none());
    }

    #[test]
    fn published_rename_replays_idempotently_after_crash() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        // First checkpoint gives commits a WAL home.
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("baseline flush should succeed");
        // Rename published after the checkpoint: only the WAL covers it.
        table
            .rename_property("weight", "mass")
            .expect("rename should publish");
        assert!(!table.properties.has_property("weight"));
        drop(table);

        let mut recovered = make_table();
        recovered.load(dir.path()).expect("load replays the rename");
        assert!(!recovered.properties.has_property("weight"));
        assert!(recovered.properties.has_property("mass"));

        // The WAL is not truncated by a load: a second replay must skip the
        // already-applied rename and land in the same state.
        let mut again = make_table();
        again
            .load(dir.path())
            .expect("second replay must stay idempotent");
        assert!(!again.properties.has_property("weight"));
        assert!(again.properties.has_property("mass"));
    }
}

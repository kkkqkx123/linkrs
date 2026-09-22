//! Staged drop-column state machine for edge property columns.
//!
//! Dropping a column moves through explicit states so every step can roll
//! back: `prepare` validates and snapshots the doomed column without touching
//! storage, `durable` marks the prepared snapshot checkpoint-durable, and
//! `publish` removes the physical column and the schema entry and records
//! history (restoring from the snapshot when history recording fails).
//! `abort` drops the pending change before anything is removed. Pending state
//! lives only in memory: a crash before publishing is equivalent to an abort
//! because reload rebuilds the property store from the published schema.

use graphdb_core::{StorageError, StorageResult};

use super::core::EdgeStore;
use crate::edge::property_schema::PropertySchema;
use crate::schema::ChangeDetails;
use crate::types::StoragePropertyDef;
use crate::vertex::column::Column;

/// Lifecycle state of one pending drop-column change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingDropColumnState {
    /// Validated and snapshotted, nothing removed.
    Prepared,
    /// Prepared snapshot covered by a checkpoint; safe to publish durably.
    Durable,
}

/// One in-flight drop-column change.
///
/// The backup is snapshotted at prepare time so a failed publish restores
/// the exact schema position, column contents and name-cache entry.
#[derive(Debug, Clone)]
pub struct PendingDropColumn {
    pub name: String,
    pub schema_index: usize,
    pub cached_index: Option<usize>,
    pub schema_backup: PropertySchema,
    pub schema_def_backup: StoragePropertyDef,
    pub column_backup: Column,
    pub had_column_dirt: bool,
    pub state: PendingDropColumnState,
}

impl EdgeStore {
    /// Borrow the pending drop-column change, if any.
    pub fn pending_drop_column(&self) -> Option<&PendingDropColumn> {
        self.pending_drop_column.as_ref()
    }

    /// Validate a drop-column request and snapshot the doomed column.
    ///
    /// Touches nothing: the schema, the physical column and history stay
    /// unchanged until publishing.
    pub fn prepare_drop_property(&mut self, name: &str) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if matches!(
            self.schema.record_form,
            crate::edge::RecordForm::Pure | crate::edge::RecordForm::Bundled
        ) {
            return Err(StorageError::invalid_operation(
                crate::edge::INLINE_FORM_SCHEMA_CHANGE_MSG.to_string(),
            ));
        }
        if self.pending_add_column.is_some()
            || self.pending_drop_column.is_some()
            || self.pending_rename_column.is_some()
        {
            return Err(StorageError::invalid_operation(
                crate::edge::SCHEMA_CHANGE_PENDING_MSG.to_string(),
            ));
        }
        let schema_index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let column_index = self
            .properties
            .property_schema()
            .iter()
            .position(|schema| schema.name == name)
            .ok_or_else(|| StorageError::column_not_found(name.to_string()))?;
        let Some(column_backup) = self.properties.column_cloned(name) else {
            return Err(StorageError::column_not_found(name.to_string()));
        };
        let schema_backup = self.properties.property_schema()[column_index].clone();
        let schema_def_backup = self.schema.properties[schema_index].clone();
        let cached_index = self.property_index_cache.get(name).copied();
        let had_column_dirt = self.properties.has_column_dirt(name);
        self.pending_drop_column = Some(PendingDropColumn {
            name: name.to_string(),
            schema_index,
            cached_index,
            schema_backup,
            schema_def_backup,
            column_backup,
            had_column_dirt,
            state: PendingDropColumnState::Prepared,
        });
        Ok(())
    }

    /// Mark the prepared drop checkpoint-durable.
    ///
    /// The prepared snapshot touches no storage, so any checkpoint after
    /// preparing covers it; publishing from `Durable` is the durable path
    /// while publishing directly from `Prepared` stays available for
    /// memory-only immediate drops.
    pub fn durable_pending_drop_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_drop_column.as_ref().ok_or_else(|| {
            StorageError::invalid_operation(
                "no pending drop-column change to mark durable".to_string(),
            )
        })?;
        if pending.state != PendingDropColumnState::Prepared {
            return Err(StorageError::invalid_operation(
                "pending drop-column change is already durable".to_string(),
            ));
        }
        if let Some(pending) = self.pending_drop_column.as_mut() {
            pending.state = PendingDropColumnState::Durable;
        }
        Ok(())
    }

    /// Execute the prepared drop: physical column, schema entry, history.
    ///
    /// A history failure restores the snapshotted column, schema entry and
    /// name cache, leaving the table as if publishing never ran. The pending
    /// slot is kept on failure so the caller can retry or abort.
    pub fn publish_pending_drop_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_drop_column.clone().ok_or_else(|| {
            StorageError::invalid_operation("no pending drop-column change to publish".to_string())
        })?;
        if let Some(dir) = self.wal_dir.clone() {
            super::wal::append_ops(
                &dir,
                &[super::wal::EdgeWalOp::SchemaDrop {
                    name: pending.name.clone(),
                }],
            )?;
        }
        self.properties.remove_property(&pending.name)?;
        self.schema.properties.remove(pending.schema_index);
        self.property_index_cache.remove(&pending.name);
        for idx in self.property_index_cache.values_mut() {
            if *idx > pending.schema_index {
                *idx -= 1;
            }
        }
        if let Err(error) = self.record_schema_change(ChangeDetails::PropertyRemoved {
            name: pending.schema_backup.name.clone(),
            data_type: pending.schema_backup.data_type.clone(),
        }) {
            self.restore_dropped_column(&pending);
            return Err(error);
        }
        self.pending_drop_column = None;
        // Column removal changes rows in every owner: trace them all and
        // forget the dropped column in the per-group scopes.
        self.forget_property_column_in_dirt(&pending.name);
        self.trace_all_owner_groups_for_columns(&[]);
        Ok(())
    }

    /// Restore a drop publish that failed after mutating storage.
    fn restore_dropped_column(&mut self, pending: &PendingDropColumn) {
        self.properties.restore_property_at(
            pending.schema_index,
            pending.schema_backup.clone(),
            pending.column_backup.clone(),
            pending.had_column_dirt,
        );
        if pending.schema_index <= self.schema.properties.len() {
            self.schema
                .properties
                .insert(pending.schema_index, pending.schema_def_backup.clone());
        }
        self.rebuild_property_index_cache();
    }

    /// Rebuild the property name cache from schema positions.
    fn rebuild_property_index_cache(&mut self) {
        self.property_index_cache.clear();
        for (idx, prop) in self.schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
    }

    /// Drop the pending change. Nothing was removed yet, so there is nothing
    /// to restore: prepared drops built only the in-memory snapshot.
    pub fn abort_pending_drop_property(&mut self) -> StorageResult<()> {
        self.pending_drop_column.take().ok_or_else(|| {
            StorageError::invalid_operation("no pending drop-column change to abort".to_string())
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
            properties: vec![
                StoragePropertyDef::new(
                    "weight".to_string(),
                    graphdb_core::types::DataType::Double,
                ),
                StoragePropertyDef {
                    nullable: true,
                    ..StoragePropertyDef::new(
                        "score".to_string(),
                        graphdb_core::types::DataType::Int,
                    )
                },
            ],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds")
    }

    #[test]
    fn staged_drop_column_full_lifecycle() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert should succeed");
        table
            .prepare_drop_property("score")
            .expect("prepare should succeed");
        assert!(table.properties.has_property("score"));
        table
            .publish_pending_drop_property()
            .expect("publish should succeed");
        assert!(table.pending_drop_column().is_none());
        assert!(!table.properties.has_property("score"));
        assert!(!table.schema.properties.iter().any(|p| p.name == "score"));
        assert!(table.properties.has_property("weight"));
        assert!(table.has_edge(0, 1, 0, 200));
    }

    #[test]
    fn abort_after_prepare_leaves_no_trace() {
        let mut table = make_table();
        table
            .prepare_drop_property("score")
            .expect("prepare should succeed");
        table
            .abort_pending_drop_property()
            .expect("abort should succeed");
        assert!(table.pending_drop_column().is_none());
        assert!(table.properties.has_property("score"));
        assert!(table.schema.properties.iter().any(|p| p.name == "score"));
        table
            .prepare_drop_property("score")
            .expect("re-prepare should succeed");
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut table = make_table();
        assert!(table.publish_pending_drop_property().is_err());
        assert!(table.abort_pending_drop_property().is_err());
        assert!(table.prepare_drop_property("missing").is_err());
        table
            .prepare_drop_property("score")
            .expect("first prepare should succeed");
        assert!(table.prepare_drop_property("weight").is_err());
        assert!(table
            .prepare_add_property("extra".to_string(), graphdb_core::DataType::Int, true, None)
            .is_err());
    }

    #[test]
    fn immediate_drop_uses_the_staged_path() {
        let mut table = make_table();
        table.remove_property("score").expect("drop should succeed");
        assert!(table.pending_drop_column().is_none());
        assert!(!table.properties.has_property("score"));
        assert!(!table.schema.properties.iter().any(|p| p.name == "score"));
        assert!(table.remove_property("score").is_err());
    }

    #[test]
    fn restore_puts_back_exact_schema_position() {
        let mut table = make_table();
        table
            .prepare_drop_property("weight")
            .expect("prepare should succeed");
        let pending = table
            .pending_drop_column()
            .cloned()
            .expect("pending exists");
        // Simulate the publish-time removal that precedes a history failure.
        table
            .properties
            .remove_property("weight")
            .expect("removal runs");
        table.schema.properties.remove(0);
        table.restore_dropped_column(&pending);
        assert_eq!(table.schema.properties[0].name, "weight");
        assert_eq!(table.schema.properties[1].name, "score");
        assert_eq!(table.property_index_cache.get("weight"), Some(&0));
        assert_eq!(table.property_index_cache.get("score"), Some(&1));
        assert!(table.properties.has_property("weight"));
    }

    #[test]
    fn prepared_drop_does_not_survive_reload() {
        let mut table = make_table();
        table
            .prepare_drop_property("score")
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
        assert!(loaded.properties.has_property("score"));
        assert!(loaded.schema.properties.iter().any(|p| p.name == "score"));
        assert!(loaded.pending_drop_column().is_none());
    }

    #[test]
    fn published_drop_survives_reload() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert should succeed");
        table.remove_property("score").expect("drop should succeed");
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");
        let mut loaded = make_table();
        loaded.load(dir.path()).expect("load should succeed");
        assert!(!loaded.properties.has_property("score"));
        assert!(loaded.has_edge(0, 1, 0, 200));
    }
}

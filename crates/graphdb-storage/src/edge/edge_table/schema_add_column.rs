//! Staged add-column state machine for edge property columns.
//!
//! Adding a column moves through explicit states so every step can roll back:
//! `prepare` validates without touching storage, `fill` builds the physical
//! column (backfilling the default when one is given), `durable` marks the
//! filled column checkpoint-durable after a checkpoint has flushed it, and
//! `publish` activates the schema entry and records history. `abort` drops
//! the pending change at any point before publishing. Pending state lives
//! only in memory: a crash before publishing is equivalent to an abort
//! because reload rebuilds the property store from the published schema.

use graphdb_core::{DataType, StorageError, StorageResult, Value};

use super::core::EdgeStore;
use crate::schema::ChangeDetails;
use crate::types::StoragePropertyDef;

/// Lifecycle state of one pending add-column change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingAddColumnState {
    /// Validated, no storage touched.
    Prepared,
    /// Physical column built (and default backfilled when given).
    Filled,
    /// Filled column flushed by a checkpoint; safe to publish durably.
    Durable,
}

/// One in-flight add-column change.
#[derive(Debug, Clone)]
pub struct PendingAddColumn {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
    pub default_value: Option<Value>,
    pub state: PendingAddColumnState,
}

impl EdgeStore {
    /// Borrow the pending add-column change, if any.
    pub fn pending_add_column(&self) -> Option<&PendingAddColumn> {
        self.pending_add_column.as_ref()
    }

    /// Validate an add-column request without touching storage or schema.
    pub fn prepare_add_property(
        &mut self,
        name: String,
        data_type: DataType,
        nullable: bool,
        default_value: Option<Value>,
    ) -> StorageResult<()> {
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
        if self.properties.has_property(&name) {
            return Err(StorageError::column_already_exists(name));
        }
        if self.schema.properties.iter().any(|prop| prop.name == name) {
            return Err(StorageError::column_already_exists(name));
        }
        if let Some(default) = &default_value {
            if default.data_type() != data_type {
                return Err(StorageError::type_mismatch(data_type, default.data_type()));
            }
        }
        self.pending_add_column = Some(PendingAddColumn {
            name,
            data_type,
            nullable,
            default_value,
            state: PendingAddColumnState::Prepared,
        });
        Ok(())
    }

    /// Build the physical column for the prepared change.
    ///
    /// Uses the single column-construction implementation shared with the
    /// single-step path.
    pub fn fill_pending_add_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_add_column.as_ref().ok_or_else(|| {
            StorageError::invalid_operation("no pending add-column change to fill".to_string())
        })?;
        if pending.state != PendingAddColumnState::Prepared {
            return Err(StorageError::invalid_operation(
                "pending add-column change is already filled".to_string(),
            ));
        }
        let name = pending.name.clone();
        let data_type = pending.data_type.clone();
        let nullable = pending.nullable;
        let default_value = pending.default_value.clone();
        self.properties
            .add_property(name.clone(), data_type, nullable)?;
        if let Some(default) = &default_value {
            if let Err(error) = self.properties.backfill_column(&name, default) {
                let _ = self.properties.remove_property(&name);
                return Err(error);
            }
        }
        if let Some(pending) = self.pending_add_column.as_mut() {
            pending.state = PendingAddColumnState::Filled;
        }
        // The backfill touches rows in every owner group: trace them all so
        // no clean group is skipped, with the new column as patch scope.
        self.trace_all_owner_groups_for_columns(std::slice::from_ref(&name));
        Ok(())
    }

    /// Mark the filled change checkpoint-durable.
    ///
    /// Requires a checkpoint since the fill: the pending column must carry no
    /// dirt and the table must carry no property dirt, proving the filled
    /// column reached the last checkpoint. Publishing from `Durable` is the
    /// durable path; publishing directly from `Filled` stays available for
    /// memory-only immediate adds.
    pub fn durable_pending_add_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_add_column.as_ref().ok_or_else(|| {
            StorageError::invalid_operation(
                "no pending add-column change to mark durable".to_string(),
            )
        })?;
        if pending.state != PendingAddColumnState::Filled {
            return Err(StorageError::invalid_operation(
                "pending add-column change must be filled before marking durable".to_string(),
            ));
        }
        if self.properties_dirty || self.properties.has_column_dirt(&pending.name) {
            return Err(StorageError::invalid_operation(
                "checkpoint required before marking add-column durable".to_string(),
            ));
        }
        if let Some(pending) = self.pending_add_column.as_mut() {
            pending.state = PendingAddColumnState::Durable;
        }
        Ok(())
    }

    /// Activate the filled change: schema entry, name cache, history record.
    ///
    /// Publishing is the durability boundary; the pending slot is cleared so
    /// a later crash replays the published schema only.
    pub fn publish_pending_add_property(&mut self) -> StorageResult<()> {
        let (name, data_type, nullable, default_value) = match self.pending_add_column.as_ref() {
            Some(pending)
                if pending.state == PendingAddColumnState::Filled
                    || pending.state == PendingAddColumnState::Durable =>
            {
                (
                    pending.name.clone(),
                    pending.data_type.clone(),
                    pending.nullable,
                    pending.default_value.clone(),
                )
            }
            Some(_) => {
                return Err(StorageError::invalid_operation(
                    "pending add-column change must be filled before publishing".to_string(),
                ));
            }
            None => {
                return Err(StorageError::invalid_operation(
                    "no pending add-column change to publish".to_string(),
                ));
            }
        };
        if let Some(dir) = self.wal_dir.clone() {
            super::wal::append_ops(
                &dir,
                &[super::wal::EdgeWalOp::SchemaAdd {
                    name: name.clone(),
                    data_type: data_type.clone(),
                    nullable,
                    default: default_value.clone(),
                }],
            )?;
        }
        let prop_def = StoragePropertyDef::new(name.clone(), data_type.clone());
        let new_idx = self.schema.properties.len();
        self.schema.properties.push(prop_def);
        self.property_index_cache.insert(name.clone(), new_idx);
        if let Err(error) = self.record_schema_change(ChangeDetails::PropertyAdded {
            name: name.clone(),
            data_type,
            nullable,
            default_value,
        }) {
            self.schema.properties.pop();
            self.property_index_cache.remove(&name);
            return Err(error);
        }
        self.pending_add_column = None;
        self.mark_properties_dirty();
        Ok(())
    }

    /// Drop the pending change, removing the physical column when filled.
    ///
    /// Never touches published schema or history: prepared changes built
    /// nothing, filled and durable changes only built the unpublished
    /// physical column.
    pub fn abort_pending_add_property(&mut self) -> StorageResult<()> {
        let pending = self.pending_add_column.take().ok_or_else(|| {
            StorageError::invalid_operation("no pending add-column change to abort".to_string())
        })?;
        if pending.state == PendingAddColumnState::Filled
            || pending.state == PendingAddColumnState::Durable
        {
            self.properties.remove_property(&pending.name)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::edge_table::iterator::EdgeTableScanIterator;
    use crate::edge::{EdgeRecord, EdgeSchema, EdgeStrategy, RecordForm};
    use crate::types::StoragePropertyDef;

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
    fn pending_conflict_uses_the_shared_wording_everywhere() {
        let mut table = make_table();
        table
            .prepare_add_property("score".to_string(), DataType::Int, true, None)
            .expect("first prepare succeeds");
        // Add, drop and rename share one pending slot: every second prepare
        // reports the same conflict from the same constant.
        let drop_err = table
            .prepare_drop_property("weight")
            .expect_err("second staged change must be rejected");
        let rename_err = table
            .prepare_rename_property("weight", "mass")
            .expect_err("second staged change must be rejected");
        assert!(
            drop_err
                .to_string()
                .contains(crate::edge::SCHEMA_CHANGE_PENDING_MSG),
            "unexpected wording: {}",
            drop_err
        );
        assert!(
            rename_err
                .to_string()
                .contains(crate::edge::SCHEMA_CHANGE_PENDING_MSG),
            "unexpected wording: {}",
            rename_err
        );
    }

    #[test]
    fn staged_add_column_full_lifecycle() {
        let mut table = make_table();
        table
            .prepare_add_property(
                "score".to_string(),
                DataType::Int,
                false,
                Some(Value::Int(0)),
            )
            .expect("prepare should succeed");
        assert_eq!(
            table.pending_add_column().expect("pending exists").state,
            PendingAddColumnState::Prepared
        );
        assert!(!table.properties.has_property("score"));
        table
            .fill_pending_add_property()
            .expect("fill should succeed");
        assert!(table.properties.has_property("score"));
        table
            .publish_pending_add_property()
            .expect("publish should succeed");
        assert!(table.pending_add_column().is_none());
        assert!(table.schema.properties.iter().any(|p| p.name == "score"));
        assert!(table.property_index_cache.contains_key("score"));
        table
            .insert_edge(
                0,
                1,
                0,
                &[
                    ("weight".to_string(), Value::Double(1.0)),
                    ("score".to_string(), Value::Int(9)),
                ],
                100,
            )
            .expect("insert with new column should succeed");
        let rows = table.properties.row_count();
        assert!(rows >= 1);
    }

    #[test]
    fn abort_after_prepare_leaves_no_trace() {
        let mut table = make_table();
        table
            .prepare_add_property("score".to_string(), DataType::Int, true, None)
            .expect("prepare should succeed");
        table
            .abort_pending_add_property()
            .expect("abort should succeed");
        assert!(table.pending_add_column().is_none());
        assert!(!table.properties.has_property("score"));
        assert!(!table.schema.properties.iter().any(|p| p.name == "score"));
        // The name is reusable after an abort.
        table
            .prepare_add_property("score".to_string(), DataType::Int, true, None)
            .expect("re-prepare should succeed");
    }

    #[test]
    fn abort_after_fill_removes_physical_column() {
        let mut table = make_table();
        table
            .prepare_add_property("score".to_string(), DataType::Int, true, None)
            .expect("prepare should succeed");
        table
            .fill_pending_add_property()
            .expect("fill should succeed");
        assert!(table.properties.has_property("score"));
        table
            .abort_pending_add_property()
            .expect("abort should succeed");
        assert!(!table.properties.has_property("score"));
        assert!(!table.schema.properties.iter().any(|p| p.name == "score"));
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut table = make_table();
        assert!(table.fill_pending_add_property().is_err());
        assert!(table.publish_pending_add_property().is_err());
        assert!(table.abort_pending_add_property().is_err());
        table
            .prepare_add_property("score".to_string(), DataType::Int, true, None)
            .expect("first prepare should succeed");
        assert!(table
            .prepare_add_property("other".to_string(), DataType::Int, true, None)
            .is_err());
        // Publishing before filling is rejected.
        assert!(table.publish_pending_add_property().is_err());
        table
            .fill_pending_add_property()
            .expect("fill should succeed");
        // Filling twice is rejected.
        assert!(table.fill_pending_add_property().is_err());
        // Duplicate names are rejected at prepare time.
        let mut dup = make_table();
        assert!(dup
            .prepare_add_property("weight".to_string(), DataType::Double, true, None)
            .is_err());
        // A default value of the wrong type is rejected at prepare time.
        assert!(dup
            .prepare_add_property(
                "score".to_string(),
                DataType::Int,
                true,
                Some(Value::Double(1.0)),
            )
            .is_err());
    }

    #[test]
    fn default_value_backfills_existing_rows() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert should succeed");
        table
            .prepare_add_property(
                "score".to_string(),
                DataType::Int,
                false,
                Some(Value::Int(7)),
            )
            .expect("prepare should succeed");
        table
            .fill_pending_add_property()
            .expect("fill should succeed");
        table
            .publish_pending_add_property()
            .expect("publish should succeed");
        let records: Vec<EdgeRecord> = EdgeTableScanIterator::new(&table, 200).collect();
        assert_eq!(records.len(), 1);
        assert!(records[0]
            .properties
            .iter()
            .any(|(k, v)| k == "score" && v == &Value::Int(7)));
    }

    #[test]
    fn immediate_add_uses_the_staged_path() {
        let mut table = make_table();
        table
            .add_property("score".to_string(), DataType::Int, true)
            .expect("immediate add should succeed");
        assert!(table.pending_add_column().is_none());
        assert!(table.properties.has_property("score"));
        assert!(table.schema.properties.iter().any(|p| p.name == "score"));
    }
}

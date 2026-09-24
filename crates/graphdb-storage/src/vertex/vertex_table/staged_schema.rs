//! Staged schema evolution for vertex property columns.
//!
//! Mirrors the edge staged add/drop/rename state machine: `prepare`
//! validates without touching storage, `fill` builds (or validates) the
//! physical column, and `publish` activates the schema entry and records
//! history. `abort` drops the pending change at any point before publishing.
//! Pending state lives only in memory: a crash before publishing is
//! equivalent to an abort because reload rebuilds columns from the published
//! schema.

use super::core::VertexTable;
use crate::schema::ChangeDetails;
use crate::types::StoragePropertyDef;
use graphdb_core::{StorageError, StorageResult};

/// Lifecycle state of one pending vertex schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingVertexSchemaState {
    /// Validated, no storage touched.
    Prepared,
    /// Physical column built (add) or preconditions rechecked (drop/rename).
    Filled,
}

/// One in-flight vertex schema change.
#[derive(Debug, Clone)]
pub enum PendingVertexSchemaKind {
    Add(StoragePropertyDef),
    Drop { name: String },
    Rename { old_name: String, new_name: String },
}

/// One in-flight vertex schema change with its lifecycle state.
#[derive(Debug, Clone)]
pub struct PendingVertexSchemaChange {
    pub kind: PendingVertexSchemaKind,
    pub state: PendingVertexSchemaState,
}

impl VertexTable {
    /// Whether a schema change is currently staged.
    pub fn has_pending_schema_change(&self) -> bool {
        self.pending_schema_change.is_some()
    }

    fn ensure_no_pending_schema_change(&self) -> StorageResult<()> {
        if self.pending_schema_change.is_some() {
            return Err(StorageError::invalid_operation(
                "a vertex schema change is already pending".to_string(),
            ));
        }
        Ok(())
    }

    fn ensure_open(&self) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        Ok(())
    }

    /// Validate an add-column request without touching storage or schema.
    pub fn prepare_add_property_staged(&mut self, prop: StoragePropertyDef) -> StorageResult<()> {
        self.ensure_open()?;
        self.ensure_no_pending_schema_change()?;
        if self.columns.get_column(&prop.name).is_some() {
            return Err(StorageError::column_already_exists(prop.name.clone()));
        }
        self.pending_schema_change = Some(PendingVertexSchemaChange {
            kind: PendingVertexSchemaKind::Add(prop),
            state: PendingVertexSchemaState::Prepared,
        });
        Ok(())
    }

    /// Validate a drop-column request without touching storage or schema.
    pub fn prepare_remove_property_staged(&mut self, prop_name: &str) -> StorageResult<()> {
        self.ensure_open()?;
        self.ensure_no_pending_schema_change()?;
        let index = self
            .schema
            .properties
            .iter()
            .position(|prop| prop.name == prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;
        if index == self.schema.primary_key_index {
            return Err(StorageError::not_supported(
                "Removing the primary key property is not supported".to_string(),
            ));
        }
        self.pending_schema_change = Some(PendingVertexSchemaChange {
            kind: PendingVertexSchemaKind::Drop {
                name: prop_name.to_string(),
            },
            state: PendingVertexSchemaState::Prepared,
        });
        Ok(())
    }

    /// Validate a rename-column request without touching storage or schema.
    pub fn prepare_rename_property_staged(
        &mut self,
        old_name: &str,
        new_name: &str,
    ) -> StorageResult<()> {
        self.ensure_open()?;
        self.ensure_no_pending_schema_change()?;
        if self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == new_name)
        {
            return Err(StorageError::column_already_exists(new_name.to_string()));
        }
        if !self
            .schema
            .properties
            .iter()
            .any(|prop| prop.name == old_name)
        {
            return Err(StorageError::column_not_found(old_name.to_string()));
        }
        self.pending_schema_change = Some(PendingVertexSchemaChange {
            kind: PendingVertexSchemaKind::Rename {
                old_name: old_name.to_string(),
                new_name: new_name.to_string(),
            },
            state: PendingVertexSchemaState::Prepared,
        });
        Ok(())
    }

    /// Build the physical column for the prepared change.
    ///
    /// Add builds the column (same layout as the single-step path); drop and
    /// rename only recheck preconditions because their physical work happens
    /// at publish time.
    pub fn fill_pending_schema_change(&mut self) -> StorageResult<()> {
        let kind = match self.pending_schema_change.as_ref() {
            Some(pending) if pending.state == PendingVertexSchemaState::Prepared => {
                pending.kind.clone()
            }
            Some(_) => {
                return Err(StorageError::invalid_operation(
                    "pending vertex schema change is already filled".to_string(),
                ));
            }
            None => {
                return Err(StorageError::invalid_operation(
                    "no pending vertex schema change to fill".to_string(),
                ));
            }
        };
        match kind {
            PendingVertexSchemaKind::Add(prop) => {
                self.columns
                    .add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
                if let Some(col) = self.columns.get_column_mut(&prop.name) {
                    col.set_chunk_capacity(self.chunk_capacity);
                }
                if matches!(
                    prop.data_type,
                    graphdb_core::DataType::String | graphdb_core::DataType::Blob
                ) {
                    if let Some(col) = self.columns.get_column_mut(&prop.name) {
                        col.set_overflow_threshold(self.string_overflow_threshold);
                    }
                }
            }
            PendingVertexSchemaKind::Drop { name } => {
                if self.columns.get_column(&name).is_none() {
                    return Err(StorageError::column_not_found(name));
                }
            }
            PendingVertexSchemaKind::Rename { old_name, .. } => {
                if self.columns.get_column(&old_name).is_none() {
                    return Err(StorageError::column_not_found(old_name));
                }
            }
        }
        if let Some(pending) = self.pending_schema_change.as_mut() {
            pending.state = PendingVertexSchemaState::Filled;
        }
        Ok(())
    }

    /// Activate the filled change: schema entry, name cache, history record.
    ///
    /// Publishing is the visibility boundary; the pending slot is cleared so
    /// a later crash replays the published schema only.
    pub fn publish_pending_schema_change(&mut self) -> StorageResult<()> {
        let kind = match self.pending_schema_change.as_ref() {
            Some(pending) if pending.state == PendingVertexSchemaState::Filled => {
                pending.kind.clone()
            }
            Some(_) => {
                return Err(StorageError::invalid_operation(
                    "pending vertex schema change must be filled before publishing".to_string(),
                ));
            }
            None => {
                return Err(StorageError::invalid_operation(
                    "no pending vertex schema change to publish".to_string(),
                ));
            }
        };
        match kind {
            PendingVertexSchemaKind::Add(prop) => {
                self.schema.properties.push(prop.clone());
                let idx = self.schema.properties.len() - 1;
                self.property_index_cache.insert(prop.name.clone(), idx);
                if let Err(error) = self.record_schema_change(ChangeDetails::PropertyAdded {
                    name: prop.name.clone(),
                    data_type: prop.data_type.clone(),
                    nullable: prop.nullable,
                    default_value: prop.default_value.clone(),
                }) {
                    self.schema.properties.pop();
                    self.property_index_cache.remove(&prop.name);
                    return Err(error);
                }
            }
            PendingVertexSchemaKind::Drop { name } => {
                let index = self
                    .schema
                    .properties
                    .iter()
                    .position(|prop| prop.name == name)
                    .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
                if index == self.schema.primary_key_index {
                    return Err(StorageError::not_supported(
                        "Removing the primary key property is not supported".to_string(),
                    ));
                }
                let removed_prop = self.schema.properties[index].clone();
                self.columns.remove_column(&name)?;
                self.schema.properties.remove(index);
                if index < self.schema.primary_key_index {
                    self.schema.primary_key_index -= 1;
                }
                self.property_index_cache.remove(&name);
                for idx in self.property_index_cache.values_mut() {
                    if *idx > index {
                        *idx -= 1;
                    }
                }
                self.record_schema_change(ChangeDetails::PropertyRemoved {
                    name: removed_prop.name,
                    data_type: removed_prop.data_type,
                })?;
            }
            PendingVertexSchemaKind::Rename { old_name, new_name } => {
                if self
                    .schema
                    .properties
                    .iter()
                    .any(|prop| prop.name == new_name)
                {
                    return Err(StorageError::column_already_exists(new_name.clone()));
                }
                let index = self
                    .schema
                    .properties
                    .iter()
                    .position(|prop| prop.name == old_name)
                    .ok_or_else(|| StorageError::column_not_found(old_name.clone()))?;
                self.columns.rename_column(&old_name, new_name.clone())?;
                self.schema.properties[index].name = new_name.clone();
                if let Some(idx) = self.property_index_cache.remove(&old_name) {
                    self.property_index_cache.insert(new_name.clone(), idx);
                }
                self.record_schema_change(ChangeDetails::PropertyRenamed { old_name, new_name })?;
            }
        }
        self.pending_schema_change = None;
        Ok(())
    }

    /// Drop the pending change, removing the physical column when filled.
    ///
    /// Never touches published schema or history: prepared changes built
    /// nothing, filled add changes only built the unpublished column.
    pub fn abort_pending_schema_change(&mut self) -> StorageResult<()> {
        let pending = self.pending_schema_change.take().ok_or_else(|| {
            StorageError::invalid_operation("no pending vertex schema change to abort".to_string())
        })?;
        if pending.state == PendingVertexSchemaState::Filled {
            if let PendingVertexSchemaKind::Add(prop) = &pending.kind {
                self.columns.remove_column(&prop.name)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vertex::vertex_table::core::VertexTableConfig;
    use crate::vertex::{LabelId, VertexSchema};
    use graphdb_core::DataType;

    fn test_schema() -> VertexSchema {
        VertexSchema {
            label_id: 0,
            label_name: "person".to_string(),
            properties: vec![StoragePropertyDef::new("id".to_string(), DataType::Int)],
            primary_key_index: 0,
            schema_version: 1,
        }
    }

    fn new_table() -> VertexTable {
        VertexTable::with_config(
            0,
            "person".to_string(),
            test_schema(),
            VertexTableConfig::default(),
        )
    }

    fn age_prop() -> StoragePropertyDef {
        StoragePropertyDef {
            name: "age".to_string(),
            data_type: DataType::Int,
            nullable: true,
            default_value: None,
        }
    }

    fn apply_add(table: &mut VertexTable, prop: StoragePropertyDef) {
        table.prepare_add_property_staged(prop).expect("prepare");
        table.fill_pending_schema_change().expect("fill");
        table.publish_pending_schema_change().expect("publish");
    }

    #[test]
    fn staged_add_becomes_visible_only_at_publish() {
        let mut table = new_table();
        table
            .prepare_add_property_staged(age_prop())
            .expect("prepare");
        assert!(table.has_pending_schema_change());
        assert!(table.columns.get_column("age").is_none());
        table.fill_pending_schema_change().expect("fill");
        assert!(table.schema.properties.iter().all(|p| p.name != "age"));
        table.publish_pending_schema_change().expect("publish");
        assert!(!table.has_pending_schema_change());
        assert!(table.columns.get_column("age").is_some());
        assert!(table.schema.properties.iter().any(|p| p.name == "age"));
    }

    #[test]
    fn abort_add_removes_filled_column() {
        let mut table = new_table();
        table
            .prepare_add_property_staged(age_prop())
            .expect("prepare");
        table.fill_pending_schema_change().expect("fill");
        table.abort_pending_schema_change().expect("abort");
        assert!(!table.has_pending_schema_change());
        assert!(table.columns.get_column("age").is_none());
        assert!(table.schema.properties.iter().all(|p| p.name != "age"));
    }

    #[test]
    fn staged_drop_and_rename_roundtrip() {
        let mut table = new_table();
        apply_add(&mut table, age_prop());
        table
            .prepare_remove_property_staged("age")
            .expect("prepare drop");
        table.fill_pending_schema_change().expect("fill drop");
        table.publish_pending_schema_change().expect("publish drop");
        assert!(table.columns.get_column("age").is_none());

        apply_add(&mut table, age_prop());
        table
            .prepare_rename_property_staged("age", "years")
            .expect("prepare rename");
        table.fill_pending_schema_change().expect("fill rename");
        table
            .publish_pending_schema_change()
            .expect("publish rename");
        assert!(table.columns.get_column("years").is_some());
        assert!(table.columns.get_column("age").is_none());
    }

    #[test]
    fn second_change_rejected_while_pending() {
        let mut table = new_table();
        table
            .prepare_add_property_staged(age_prop())
            .expect("prepare");
        assert!(table.prepare_add_property_staged(age_prop()).is_err());
        assert!(table.prepare_remove_property_staged("id").is_err());
        assert!(table.prepare_rename_property_staged("id", "other").is_err());
        let mut schema = test_schema();
        schema.properties.push(age_prop());
        assert!(table.set_schema(schema).is_err());
        assert!(table.has_pending_schema_change());
    }

    #[test]
    fn publish_without_fill_rejected() {
        let mut table = new_table();
        table
            .prepare_add_property_staged(age_prop())
            .expect("prepare");
        assert!(table.publish_pending_schema_change().is_err());
        assert!(table.columns.get_column("age").is_none());
    }

    #[test]
    fn drop_primary_key_rejected() {
        let mut table = new_table();
        assert!(table.prepare_remove_property_staged("id").is_err());
        assert!(!table.has_pending_schema_change());
    }
}

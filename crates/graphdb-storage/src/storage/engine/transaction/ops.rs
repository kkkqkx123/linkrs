//! Transaction Operations
//!
//! Core transaction operations for vertex and edge manipulation.
//! These operations are used by the transaction system for insert, delete, and update operations.

use std::collections::HashMap;

use crate::core::types::{ColumnId, LabelId, Timestamp, VertexId};
use crate::core::Value;
use crate::storage::edge::UpdateEdgePropertyByOffsetParams;
use crate::transaction::codec::{bytes_to_value, property_value_to_value};
use crate::transaction::insert_transaction::{InsertTransactionError, InsertTransactionResult};
use crate::transaction::undo_log::{PropertyValue, UndoLogError, UndoLogResult};

use crate::storage::edge::EdgeTable;
use crate::storage::engine::data_store::EdgeTableKey;
use crate::storage::vertex::VertexTable;

/// Parameters for add_edge operation
pub struct AddEdgeParams {
    pub src_label: LabelId,
    pub src_vid: u32,
    pub dst_label: LabelId,
    pub dst_vid: u32,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for delete_edge operation
pub struct DeleteEdgeParams {
    pub src_label: LabelId,
    pub src_vid: u32,
    pub dst_label: LabelId,
    pub dst_vid: u32,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for update_edge_property_undo operation
pub struct UpdateEdgePropertyUndoParams {
    pub src_label: LabelId,
    pub src_vid: u32,
    pub dst_label: LabelId,
    pub dst_vid: u32,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for revert_delete_edge operation
pub struct RevertDeleteEdgeParams {
    pub src_label: LabelId,
    pub src_vid: u32,
    pub dst_label: LabelId,
    pub dst_vid: u32,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for delete_edge_type operation
pub struct DeleteEdgeTypeParams {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
}

/// Parameters identifying an edge type by label names
pub struct EdgeTypeLabelParams<'a> {
    pub src_label: &'a str,
    pub dst_label: &'a str,
    pub edge_label: &'a str,
}

pub struct TransactionOps;

impl TransactionOps {
    /// Resolve an external VertexId to an internal row ID.
    pub fn resolve_vertex_id(table: &VertexTable, vid: VertexId, ts: Timestamp) -> Option<u32> {
        if let Some(int_id) = vid.as_int64() {
            table.get_internal_id_by_i64(int_id, ts)
        } else if let Some(str_id) = vid.as_str() {
            table.get_internal_id(str_id, ts)
        } else {
            None
        }
    }
    pub fn add_vertex(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Vec<u8>)],
        ts: Timestamp,
    ) -> InsertTransactionResult<VertexId> {
        let props: Vec<(String, Value)> = properties
            .iter()
            .filter_map(|(k, v)| bytes_to_value(v).map(|val| (k.clone(), val)))
            .collect();

        let table = vertex_tables
            .get_mut(&label)
            .ok_or(InsertTransactionError::LabelNotFound(label))?;

        let internal_id = if let Some(int_id) = vid.as_int64() {
            table
                .insert_by_i64(int_id, &props, ts)
                .map_err(|e| InsertTransactionError::SchemaError(e.to_string()))?
        } else if let Some(str_id) = vid.as_str() {
            table
                .insert(str_id, &props, ts)
                .map_err(|e| InsertTransactionError::SchemaError(e.to_string()))?
        } else {
            return Err(InsertTransactionError::SerializationError(
                "Invalid VertexId: neither int64 nor string".to_string(),
            ));
        };

        Ok(VertexId::from_int64(internal_id as i64))
    }

    pub fn add_edge(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        vertex_tables: &HashMap<LabelId, VertexTable>,
        params: AddEdgeParams,
        properties: &[(String, Vec<u8>)],
        ts: Timestamp,
    ) -> InsertTransactionResult<()> {
        let src_table = vertex_tables
            .get(&params.src_label)
            .ok_or(InsertTransactionError::LabelNotFound(params.src_label))?;
        let dst_table = vertex_tables
            .get(&params.dst_label)
            .ok_or(InsertTransactionError::LabelNotFound(params.dst_label))?;

        let src_external = src_table.get_external_id(params.src_vid, ts).ok_or(
            InsertTransactionError::VertexNotFound(VertexId::from_int64(params.src_vid as i64)),
        )?;
        let dst_external = dst_table.get_external_id(params.dst_vid, ts).ok_or(
            InsertTransactionError::VertexNotFound(VertexId::from_int64(params.dst_vid as i64)),
        )?;

        let props: Vec<(String, Value)> = properties
            .iter()
            .filter_map(|(k, v)| bytes_to_value(v).map(|val| (k.clone(), val)))
            .collect();

        let _src_id_str = match &src_external {
            crate::storage::vertex::IdKey::Text(s) => s.clone(),
            crate::storage::vertex::IdKey::Int(i) => i.to_string(),
        };
        let _dst_id_str = match &dst_external {
            crate::storage::vertex::IdKey::Text(s) => s.clone(),
            crate::storage::vertex::IdKey::Int(i) => i.to_string(),
        };

        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        let edge_table = edge_tables
            .get_mut(&key)
            .ok_or(InsertTransactionError::LabelNotFound(params.edge_label))?;

        edge_table
            .insert_edge(params.src_vid, params.dst_vid, params.rank, &props, ts)
            .map_err(|e| InsertTransactionError::SchemaError(e.to_string()))?;

        Ok(())
    }

    pub fn delete_vertex_type(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        vertex_label_names: &mut HashMap<String, LabelId>,
        edge_label_names: &mut HashMap<String, LabelId>,
        label: LabelId,
    ) -> UndoLogResult<()> {
        let label_name = vertex_tables
            .get(&label)
            .map(|t| t.label_name().to_string());

        if let Some(name) = label_name {
            vertex_label_names.remove(&name);
        }

        vertex_tables.remove(&label);

        let mut removed_edge_keys = Vec::new();
        for (key, table) in &*edge_tables {
            if key.src_label == label || key.dst_label == label {
                let edge_name = table.label_name().to_string();
                edge_label_names.remove(&edge_name);
                removed_edge_keys.push(*key);
            }
        }
        for key in removed_edge_keys {
            edge_tables.remove(&key);
        }

        Ok(())
    }

    pub fn delete_edge_type(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        edge_label_names: &mut HashMap<String, LabelId>,
        params: DeleteEdgeTypeParams,
    ) -> UndoLogResult<()> {
        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        if let Some(table) = edge_tables.get(&key) {
            let label_name = table.label_name().to_string();
            edge_label_names.remove(&label_name);
        }
        edge_tables.remove(&key);
        Ok(())
    }

    pub fn delete_vertex(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get_mut(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id = Self::resolve_vertex_id(table, vid, ts)
            .ok_or(UndoLogError::VertexNotFound(vid))?;

        table
            .delete_by_internal_id(internal_id, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;

        Ok(())
    }

    pub fn delete_vertex_by_external_vid(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get_mut(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id =
            Self::resolve_vertex_id(table, vid, ts).ok_or(UndoLogError::LabelNotFound(0))?;

        table
            .delete_by_internal_id(internal_id, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn revert_delete_vertex(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get_mut(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id = if let Some(int_id) = vid.as_int64() {
            table.get_internal_id_by_i64_raw(int_id)
        } else if let Some(str_id) = vid.as_str() {
            table.get_internal_id_raw(str_id)
        } else {
            None
        }
        .ok_or(UndoLogError::VertexNotFound(vid))?;

        table
            .revert_delete(internal_id, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;

        Ok(())
    }

    pub fn delete_edge(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        params: DeleteEdgeParams,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        if let Some(table) = edge_tables.get_mut(&key) {
            table
                .delete_edge_by_offset(
                    params.src_vid,
                    params.dst_vid,
                    params.rank,
                    oe_offset,
                    ie_offset,
                    ts,
                )
                .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        }
        Ok(())
    }

    pub fn revert_delete_edge(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        params: RevertDeleteEdgeParams,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        if let Some(table) = edge_tables.get_mut(&key) {
            table
                .revert_delete_edge_by_offset(
                    params.src_vid,
                    params.dst_vid,
                    params.rank,
                    oe_offset,
                    ie_offset,
                    ts,
                )
                .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        }
        Ok(())
    }

    pub fn update_vertex_property_by_vid(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get_mut(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id = if let Some(int_id) = vid.as_int64() {
            table.get_internal_id_by_i64(int_id, ts)
        } else if let Some(str_id) = vid.as_str() {
            table.get_internal_id(str_id, ts)
        } else {
            None
        }
        .ok_or(UndoLogError::LabelNotFound(0))?;

        table
            .update_property(internal_id, prop_name, value, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn update_vertex_property_undo(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        label: LabelId,
        vid: VertexId,
        col_id: ColumnId,
        old_value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get_mut(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id = Self::resolve_vertex_id(table, vid, ts)
            .ok_or(UndoLogError::VertexNotFound(vid))?;

        let value = property_value_to_value(old_value);
        table
            .update_property_by_id(
                internal_id,
                col_id.0 as i32,
                &value,
                ts,
            )
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn update_edge_property(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        vertex_tables: &HashMap<LabelId, VertexTable>,
        params: crate::storage::engine::params::EdgeOperationParams,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let src_table = vertex_tables
            .get(&params.src_label)
            .ok_or(UndoLogError::LabelNotFound(params.src_label))?;
        let dst_table = vertex_tables
            .get(&params.dst_label)
            .ok_or(UndoLogError::LabelNotFound(params.dst_label))?;

        let src_internal = Self::resolve_vertex_id(src_table, params.src_id, ts)
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let dst_internal = Self::resolve_vertex_id(dst_table, params.dst_id, ts)
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        let table = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(params.edge_label))?;

        table
            .update_edge_property(
                src_internal,
                dst_internal,
                params.rank,
                prop_name,
                value,
                ts,
            )
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn update_edge_property_undo(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        params: UpdateEdgePropertyUndoParams,
        _oe_offset: i32,
        _ie_offset: i32,
        prop_id: u16,
        old_value: PropertyValue,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        let table = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let value = property_value_to_value(old_value);
        table
            .update_edge_property_by_offset(UpdateEdgePropertyByOffsetParams {
                src: params.src_vid,
                dst: params.dst_vid,
                rank: params.rank,
                prop_id,
                value,
                ts,
            })
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn create_vertex_type_undo(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        vertex_label_names: &mut HashMap<String, LabelId>,
        vertex_label_counter: &mut LabelId,
        name: &str,
    ) -> UndoLogResult<()> {
        let label = *vertex_label_counter;
        vertex_label_names.insert(name.to_string(), label);
        *vertex_label_counter = (*vertex_label_counter).max(label + 1);

        let schema = crate::storage::vertex::VertexSchema {
            label_id: label,
            label_name: name.to_string(),
            properties: Vec::new(),
            primary_key_index: 0,
            schema_version: 1,
        };

        let table = VertexTable::new(label, name.to_string(), schema);
        vertex_tables.insert(label, table);

        Ok(())
    }

    pub fn create_edge_type_undo(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        edge_label_names: &mut HashMap<String, LabelId>,
        edge_label_counter: &mut LabelId,
        vertex_tables: &HashMap<LabelId, VertexTable>,
        name: &str,
        src_label_name: &str,
        dst_label_name: &str,
    ) -> UndoLogResult<()> {
        let src_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == src_label_name)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let dst_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == dst_label_name)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let label = *edge_label_counter;
        edge_label_names.insert(name.to_string(), label);
        *edge_label_counter = (*edge_label_counter).max(label + 1);

        let schema = crate::storage::edge::EdgeSchema {
            label_id: label,
            label_name: name.to_string(),
            src_label: src_label_id,
            dst_label: dst_label_id,
            properties: Vec::new(),
            oe_strategy: crate::storage::edge::EdgeStrategy::Multiple,
            ie_strategy: crate::storage::edge::EdgeStrategy::Multiple,
            schema_version: 1,
        };

        let table = crate::storage::edge::EdgeTable::new(schema)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        let key = EdgeTableKey::new(src_label_id, dst_label_id, label);
        edge_tables.insert(key, table);

        Ok(())
    }

    pub fn revert_rename_vertex_properties(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        vertex_label_names: &mut HashMap<String, LabelId>,
        label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = vertex_label_names
            .get(label)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        if let Some(table) = vertex_tables.get_mut(&label_id) {
            let mut new_schema = table.schema().clone();
            let old_version = new_schema.schema_version;
            for (current, original) in current_names.iter().zip(original_names.iter()) {
                if let Some(prop) = new_schema
                    .properties
                    .iter_mut()
                    .find(|p| p.name == *current)
                {
                    prop.name = original.clone();
                }
            }
            table.set_schema_with_version(new_schema, old_version);
        }

        Ok(())
    }

    pub fn revert_rename_edge_properties(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        edge_label_names: &mut HashMap<String, LabelId>,
        vertex_tables: &HashMap<LabelId, VertexTable>,
        edge_labels: &EdgeTypeLabelParams,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()> {
        let src_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == edge_labels.src_label)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let dst_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == edge_labels.dst_label)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let edge_label_id = edge_label_names
            .get(edge_labels.edge_label)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let key = EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        if let Some(table) = edge_tables.get_mut(&key) {
            let mut new_schema = table.schema().clone();
            let old_version = new_schema.schema_version;
            for (current, original) in current_names.iter().zip(original_names.iter()) {
                if let Some(prop) = new_schema
                    .properties
                    .iter_mut()
                    .find(|p| p.name == *current)
                {
                    prop.name = original.clone();
                }
            }
            table.set_schema_with_version(new_schema, old_version);
        }

        Ok(())
    }

    pub fn revert_delete_vertex_properties(
        vertex_tables: &mut HashMap<LabelId, VertexTable>,
        vertex_label_names: &mut HashMap<String, LabelId>,
        label_name: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = vertex_label_names
            .get(label_name)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let table = vertex_tables
            .get_mut(&label_id)
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let mut schema = table.schema().clone();
        let old_version = schema.schema_version;
        for prop_name in prop_names {
            schema.properties.retain(|p| p.name != *prop_name);
        }

        table.set_schema_with_version(schema, old_version);

        Ok(())
    }

    pub fn revert_delete_edge_properties(
        edge_tables: &mut HashMap<EdgeTableKey, EdgeTable>,
        edge_label_names: &mut HashMap<String, LabelId>,
        vertex_tables: &HashMap<LabelId, VertexTable>,
        prop_names: &[String],
        edge_labels: &EdgeTypeLabelParams,
    ) -> UndoLogResult<()> {
        let src_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == edge_labels.src_label)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let dst_label_id = vertex_tables
            .values()
            .find(|t| t.label_name() == edge_labels.dst_label)
            .map(|t| t.label())
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let edge_label_id = edge_label_names
            .get(edge_labels.edge_label)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let key = EdgeTableKey::new(src_label_id, dst_label_id, edge_label_id);
        let table = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let mut schema = table.schema().clone();
        let old_version = schema.schema_version;
        for prop_name in prop_names {
            schema.properties.retain(|p| p.name != *prop_name);
        }

        table.set_schema_with_version(schema, old_version);

        Ok(())
    }
}



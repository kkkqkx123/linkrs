//! Transaction Operations
//!
//! Core transaction operations for vertex and edge manipulation.
//! These operations are used by the transaction system for insert, delete, and update operations.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::edge::UpdateEdgePropertyByOffsetParams;
use graphdb_core::types::{ColumnId, LabelId, Timestamp, VertexId};
use graphdb_core::Value;
use graphdb_transaction::undo_log::{UndoLogError, UndoLogResult};

use crate::edge::EdgeStore;
use crate::engine::data_store::EdgeTableKey;
use crate::vertex::ShardedVertexTable;

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
#[cfg(test)]
pub struct DeleteEdgeParams {
    pub src_label: LabelId,
    pub src_vid: u32,
    pub dst_label: LabelId,
    pub dst_vid: u32,
    pub edge_label: LabelId,
    pub rank: i64,
}

#[cfg(test)]
pub struct DeleteEdgeTypeParams {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
}

/// Parameters for update_edge_property_undo operation
pub struct UpdateEdgePropertyUndoParams {
    pub src_vid: u32,
    pub dst_vid: u32,
    pub rank: i64,
}

/// Parameters for revert_delete_edge operation
pub struct RevertDeleteEdgeParams {
    pub src_vid: u32,
    pub dst_vid: u32,
    pub rank: i64,
}

/// Parameters for delete_edge_type operation
/// Parameters identifying an edge type by label names
pub struct EdgeTypeLabelParams<'a> {
    pub src_label: &'a str,
    pub dst_label: &'a str,
    pub edge_label: &'a str,
}

pub struct TransactionOps;

impl TransactionOps {
    /// Resolve an external VertexId to an internal row ID.
    pub fn resolve_vertex_id(
        table: &ShardedVertexTable,
        vid: VertexId,
        ts: Timestamp,
    ) -> Option<u32> {
        if let Some(int_id) = vid.as_int64() {
            table.get_internal_id_by_i64(int_id, ts)
        } else if let Some(str_id) = vid.as_str() {
            table.get_internal_id(str_id, ts)
        } else {
            None
        }
    }
    pub fn add_vertex(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> UndoLogResult<VertexId> {
        let table = vertex_tables
            .get(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id = if let Some(int_id) = vid.as_int64() {
            table
                .insert_by_i64(int_id, properties, ts)
                .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?
        } else if let Some(str_id) = vid.as_str() {
            table
                .insert(str_id, properties, ts)
                .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?
        } else {
            return Err(UndoLogError::UndoFailed(
                "Invalid VertexId: neither int64 nor string".to_string(),
            ));
        };

        Ok(VertexId::from_int64(internal_id as i64))
    }

    pub fn add_edge(
        edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        params: AddEdgeParams,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let src_table = vertex_tables
            .get(&params.src_label)
            .ok_or(UndoLogError::LabelNotFound(params.src_label))?;
        let dst_table = vertex_tables
            .get(&params.dst_label)
            .ok_or(UndoLogError::LabelNotFound(params.dst_label))?;

        let _src_external =
            src_table
                .get_external_id(params.src_vid, ts)
                .ok_or(UndoLogError::VertexNotFound(VertexId::from_int64(
                    params.src_vid as i64,
                )))?;
        let _dst_external =
            dst_table
                .get_external_id(params.dst_vid, ts)
                .ok_or(UndoLogError::VertexNotFound(VertexId::from_int64(
                    params.dst_vid as i64,
                )))?;

        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        let arc = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(params.edge_label))?;
        let mut edge_table = arc.write();

        edge_table
            .insert_edge(params.src_vid, params.dst_vid, params.rank, properties, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;

        Ok(())
    }

    pub fn delete_vertex(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id =
            Self::resolve_vertex_id(table, vid, ts).ok_or(UndoLogError::VertexNotFound(vid))?;

        table
            .delete_by_internal_id(internal_id, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;

        Ok(())
    }

    pub fn delete_vertex_by_external_vid(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id =
            Self::resolve_vertex_id(table, vid, ts).ok_or(UndoLogError::LabelNotFound(0))?;

        table
            .delete_by_internal_id(internal_id, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn revert_delete_vertex(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get(&label)
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

    #[cfg(test)]
    pub fn delete_edge(
        edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
        params: DeleteEdgeParams,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let key = EdgeTableKey::new(params.src_label, params.dst_label, params.edge_label);
        if let Some(arc) = edge_tables.get_mut(&key) {
            let mut table = arc.write();
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

    pub fn update_vertex_property_by_vid(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get(&label)
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
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        label: LabelId,
        vid: VertexId,
        col_id: ColumnId,
        old_value: Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        let table = vertex_tables
            .get(&label)
            .ok_or(UndoLogError::LabelNotFound(label))?;

        let internal_id =
            Self::resolve_vertex_id(table, vid, ts).ok_or(UndoLogError::VertexNotFound(vid))?;

        table
            .update_property_by_id(internal_id, col_id.0 as i32, &old_value, ts)
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    pub fn update_edge_property(
        edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        params: crate::engine::params::EdgeOperationParams,
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
        let arc = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(params.edge_label))?;
        let mut table = arc.write();

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

    /// Single-table version of `update_edge_property_undo`.
    /// Operates on a single `EdgeStore` instead of the full catalog HashMap.
    pub fn update_edge_property_undo_single(
        table: &mut EdgeStore,
        params: UpdateEdgePropertyUndoParams,
        prop_id: u16,
        old_value: Value,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        table
            .update_edge_property_by_offset(UpdateEdgePropertyByOffsetParams {
                src: params.src_vid,
                dst: params.dst_vid,
                rank: params.rank,
                prop_id,
                value: old_value,
                ts,
            })
            .map_err(|e| UndoLogError::UndoFailed(e.to_string()))?;
        Ok(())
    }

    /// Single-table version of `revert_delete_edge`.
    /// Operates on a single `EdgeStore` instead of the full catalog HashMap.
    pub fn revert_delete_edge_single(
        table: &mut EdgeStore,
        params: RevertDeleteEdgeParams,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
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
        Ok(())
    }

    pub fn revert_rename_vertex_properties(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        vertex_label_names: &HashMap<String, LabelId>,
        label: &str,
        current_names: &[String],
        original_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = vertex_label_names
            .get(label)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        if let Some(table) = vertex_tables.get(&label_id) {
            let mut schema = table.schema();
            for (current, original) in current_names.iter().zip(original_names.iter()) {
                if let Some(prop) = schema.properties.iter_mut().find(|p| p.name == *current) {
                    prop.name = original.clone();
                }
            }
            table.apply_schema(schema);
        }

        Ok(())
    }

    pub fn revert_rename_edge_properties(
        edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
        edge_label_names: &mut HashMap<String, LabelId>,
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
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
        if let Some(arc) = edge_tables.get_mut(&key) {
            let mut table = arc.write();
            for (current, original) in current_names.iter().zip(original_names.iter()) {
                if let Some(prop) = table
                    .schema_mut()
                    .properties
                    .iter_mut()
                    .find(|p| p.name == *current)
                {
                    prop.name = original.clone();
                }
            }
        }

        Ok(())
    }

    pub fn revert_delete_vertex_properties(
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
        vertex_label_names: &HashMap<String, LabelId>,
        label_name: &str,
        prop_names: &[String],
    ) -> UndoLogResult<()> {
        let label_id = vertex_label_names
            .get(label_name)
            .copied()
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let table = vertex_tables
            .get(&label_id)
            .ok_or(UndoLogError::LabelNotFound(0))?;

        let mut schema = table.schema();
        for prop_name in prop_names {
            schema.properties.retain(|p| p.name != *prop_name);
        }
        table.apply_schema(schema);

        Ok(())
    }

    pub fn revert_delete_edge_properties(
        edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
        edge_label_names: &mut HashMap<String, LabelId>,
        vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
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
        let arc = edge_tables
            .get_mut(&key)
            .ok_or(UndoLogError::LabelNotFound(0))?;
        let mut table = arc.write();

        for prop_name in prop_names {
            table
                .schema_mut()
                .properties
                .retain(|p| p.name != *prop_name);
        }

        Ok(())
    }
}
#[cfg(test)]
pub fn delete_vertex_type(
    vertex_tables: &mut HashMap<LabelId, Arc<ShardedVertexTable>>,
    edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
    vertex_label_names: &mut HashMap<String, LabelId>,
    edge_label_names: &mut HashMap<String, LabelId>,
    label: LabelId,
) -> UndoLogResult<()> {
    if let Some(name) = vertex_tables
        .get(&label)
        .map(|table| table.label_name().to_string())
    {
        vertex_label_names.remove(&name);
    }
    vertex_tables.remove(&label);
    let keys: Vec<_> = edge_tables
        .keys()
        .filter(|key| key.src_label == label || key.dst_label == label)
        .copied()
        .collect();
    for key in keys {
        edge_tables.remove(&key);
        edge_label_names.retain(|_, edge_label| *edge_label != key.edge_label);
    }
    Ok(())
}

#[cfg(test)]
pub fn delete_edge_type(
    edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
    edge_label_names: &mut HashMap<String, LabelId>,
    params: DeleteEdgeTypeParams,
) -> UndoLogResult<()> {
    edge_tables.remove(&EdgeTableKey::new(
        params.src_label,
        params.dst_label,
        params.edge_label,
    ));
    edge_label_names.retain(|_, label| *label != params.edge_label);
    Ok(())
}

#[cfg(test)]
pub fn create_vertex_type_undo(
    vertex_tables: &mut HashMap<LabelId, Arc<ShardedVertexTable>>,
    vertex_label_names: &mut HashMap<String, LabelId>,
    vertex_label_counter: &mut LabelId,
    name: &str,
) -> UndoLogResult<()> {
    let label = *vertex_label_counter;
    vertex_label_names.insert(name.to_string(), label);
    *vertex_label_counter = (*vertex_label_counter).max(label.saturating_add(1));
    let schema = crate::vertex::VertexSchema {
        label_id: label,
        label_name: name.to_string(),
        properties: Vec::new(),
        primary_key_index: 0,
        schema_version: 1,
    };
    vertex_tables.insert(
        label,
        Arc::new(ShardedVertexTable::new(label, name.to_string(), schema)),
    );
    Ok(())
}

#[cfg(test)]
pub fn create_edge_type_undo(
    edge_tables: &mut HashMap<EdgeTableKey, Arc<RwLock<EdgeStore>>>,
    edge_label_names: &mut HashMap<String, LabelId>,
    edge_label_counter: &mut LabelId,
    vertex_tables: &HashMap<LabelId, Arc<ShardedVertexTable>>,
    name: &str,
    src_label_name: &str,
    dst_label_name: &str,
) -> UndoLogResult<()> {
    let src_label = vertex_tables
        .values()
        .find(|table| table.label_name() == src_label_name)
        .map(|t| t.label())
        .ok_or(UndoLogError::LabelNotFound(0))?;
    let dst_label = vertex_tables
        .values()
        .find(|table| table.label_name() == dst_label_name)
        .map(|t| t.label())
        .ok_or(UndoLogError::LabelNotFound(0))?;
    let label = *edge_label_counter;
    *edge_label_counter = (*edge_label_counter).max(label.saturating_add(1));
    edge_label_names.insert(name.to_string(), label);
    let schema = crate::edge::EdgeSchema {
        label_id: label,
        label_name: name.to_string(),
        src_label,
        dst_label,
        properties: Vec::new(),
        oe_strategy: crate::edge::EdgeStrategy::Multiple,
        ie_strategy: crate::edge::EdgeStrategy::Multiple,
        schema_version: 1,
    };
    edge_tables.insert(
        EdgeTableKey::new(src_label, dst_label, label),
        Arc::new(RwLock::new(
            EdgeStore::new(schema).map_err(|error| UndoLogError::UndoFailed(error.to_string()))?,
        )),
    );
    Ok(())
}

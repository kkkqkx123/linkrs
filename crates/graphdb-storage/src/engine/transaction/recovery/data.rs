use crate::engine::graph_storage::GraphStorageContext;
use crate::engine::params::EdgeOperationParams;
use crate::engine::transaction::{AddEdgeParams, TransactionOps};
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{LabelId, Timestamp, UndoLogError, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};
use graphdb_transaction::wal::{DeleteEdgeRedo, InsertEdgeRedo, UpdateEdgePropRedo};

/// Whether an edge-insert replay failure is a benign duplicate.
///
/// Replay is idempotent: a crash between commit and checkpoint replays an
/// insert the table already holds. The duplicate surfaces as an
/// already-exists failure rendered with either separator spelling, so both
/// spellings are accepted here instead of a single fragile substring.
fn is_replayable_edge_duplicate(err: &UndoLogError) -> bool {
    let text = err.to_string();
    text.contains("already exists") || text.contains("already_exists")
}

/// Whether a property-update replay failure is a benign miss.
///
/// Replay is idempotent: a second replay after a partial checkpoint, or a
/// replay racing a newer delete, meets a target that is already gone. Absent
/// labels, vertices, edges and columns are moot updates, while type mismatches
/// and other substantive failures still propagate.
fn is_benign_replay_missing(err: &UndoLogError) -> bool {
    match err {
        UndoLogError::LabelNotFound(_)
        | UndoLogError::VertexNotFound(_)
        | UndoLogError::EdgeNotFound(_)
        | UndoLogError::PropertyNotFound(_) => true,
        UndoLogError::UndoFailed(text) | UndoLogError::InvalidState(text) => {
            let folded = text.to_lowercase();
            folded.contains("not found")
                || folded.contains("missing")
                || folded.contains("no such")
                || folded.contains("does not exist")
                || folded.contains("unknown column")
                || folded.contains("column_not_found")
        }
    }
}

pub(crate) fn replay_insert_vertex(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    properties: &[(String, Value)],
    ts: Timestamp,
) -> StorageResult<()> {
    ctx.data_store().with_vertex_tables_mut(|vertex_tables| {
        if let Err(e) = TransactionOps::add_vertex(vertex_tables, label, vid, properties, ts) {
            if e.to_string().contains("already exists") {
                return Ok(());
            }
            return Err(StorageError::db_error(format!(
                "Failed to replay insert vertex: {}",
                e
            )));
        }
        Ok(())
    })?;
    ctx.mark_vertex_modified(label);
    ctx.replay_vertex_index_update(label, vid, properties, ts)?;
    Ok(())
}

pub(crate) fn replay_insert_edge(
    ctx: &GraphStorageContext,
    redo: &InsertEdgeRedo,
    ts: Timestamp,
) -> StorageResult<()> {
    let endpoints_exist = ctx.data_store().with_vertex_tables(|vertex_tables| {
        let src_exists = vertex_tables
            .get(&redo.src_label)
            .map(|t| t.as_ref())
            .and_then(|table| TransactionOps::resolve_vertex_id(table, redo.src_vid, ts))
            .is_some();

        let dst_exists = vertex_tables
            .get(&redo.dst_label)
            .map(|t| t.as_ref())
            .and_then(|table| TransactionOps::resolve_vertex_id(table, redo.dst_vid, ts))
            .is_some();
        src_exists && dst_exists
    });

    if !endpoints_exist {
        ctx.defer_edge_insert(redo.clone(), ts);
        return Ok(());
    }

    ctx.do_replay_insert_edge(redo, ts)
}

pub(crate) fn replay_delete_edge(
    ctx: &GraphStorageContext,
    redo: &DeleteEdgeRedo,
    ts: Timestamp,
) -> StorageResult<()> {
    let endpoints_exist = ctx.data_store().with_vertex_tables(|vertex_tables| {
        let src_exists = vertex_tables.contains_key(&redo.src_label)
            && resolve_external_vid(vertex_tables, redo.src_label, redo.src_vid, ts).is_some();

        let dst_exists = vertex_tables.contains_key(&redo.dst_label)
            && resolve_external_vid(vertex_tables, redo.dst_label, redo.dst_vid, ts).is_some();
        src_exists && dst_exists
    });

    if !endpoints_exist {
        ctx.defer_edge_delete(redo.clone(), ts);
        return Ok(());
    }

    ctx.do_replay_delete_edge(redo, ts)
}

pub(crate) fn replay_update_vertex_prop(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    prop_name: &str,
    value: &Value,
    ts: Timestamp,
) -> StorageResult<()> {
    let replayed = ctx
        .data_store()
        .with_vertex_tables_mut_result(|vertex_tables| {
            TransactionOps::update_vertex_property_by_vid(
                vertex_tables,
                label,
                vid,
                prop_name,
                value,
                ts,
            )
        });
    if let Err(e) = replayed {
        if !is_benign_replay_missing(&e) {
            return Err(StorageError::db_error(format!(
                "Failed to replay update vertex property: {}",
                e
            )));
        }
    }

    ctx.mark_vertex_modified(label);
    Ok(())
}

pub(crate) fn replay_update_edge_prop(
    ctx: &GraphStorageContext,
    redo: &UpdateEdgePropRedo,
    ts: Timestamp,
) -> StorageResult<()> {
    let params = EdgeOperationParams {
        src_label: redo.src_label,
        src_id: redo.src_vid,
        dst_label: redo.dst_label,
        dst_id: redo.dst_vid,
        edge_label: redo.edge_label,
        rank: redo.rank,
    };

    {
        let mut catalog = ctx.data_store().catalog_write_set();
        let replayed = TransactionOps::update_edge_property(
            &mut catalog.edge_tables,
            &catalog.vertex_tables,
            params,
            &redo.prop_name,
            &redo.value,
            ts,
        );
        if let Err(e) = replayed {
            if !is_benign_replay_missing(&e) {
                return Err(StorageError::db_error(format!(
                    "Failed to replay update edge property: {}",
                    e
                )));
            }
        }
    }
    ctx.mark_edge_modified(redo.edge_label);

    Ok(())
}

pub(crate) fn replay_delete_vertex(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    ts: Timestamp,
) -> StorageResult<()> {
    ctx.data_store().with_vertex_tables_mut(|vertex_tables| {
        let _ = TransactionOps::delete_vertex_by_external_vid(vertex_tables, label, vid, ts);
        Ok(())
    })?;
    ctx.mark_vertex_modified(label);

    ctx.replay_vertex_index_delete(label, vid, ts)?;

    Ok(())
}

pub(crate) fn resolve_external_vid(
    vertex_tables: &std::collections::HashMap<
        LabelId,
        std::sync::Arc<crate::vertex::ShardedVertexTable>,
    >,
    label: LabelId,
    vid: VertexId,
    ts: Timestamp,
) -> Option<u32> {
    let table = vertex_tables.get(&label)?;
    if let Some(int_id) = vid.as_int64() {
        table.get_internal_id_by_i64(int_id, ts)
    } else if let Some(str_id) = vid.as_str() {
        table.get_internal_id(str_id, ts)
    } else {
        None
    }
}

impl GraphStorageContext {
    pub(crate) fn do_replay_insert_edge(
        &self,
        redo: &InsertEdgeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let (src_internal, dst_internal) =
            self.data_store()
                .with_vertex_tables(|vertex_tables| -> StorageResult<(u32, u32)> {
                    let src_table = vertex_tables.get(&redo.src_label).ok_or_else(|| {
                        StorageError::db_error(format!(
                            "Source vertex label not found during recovery: label={}",
                            redo.src_label
                        ))
                    })?;
                    let dst_table = vertex_tables.get(&redo.dst_label).ok_or_else(|| {
                        StorageError::db_error(format!(
                            "Destination vertex label not found during recovery: label={}",
                            redo.dst_label
                        ))
                    })?;

                    let src_internal =
                        TransactionOps::resolve_vertex_id(src_table, redo.src_vid, ts).ok_or_else(
                            || {
                                StorageError::db_error(format!(
                                    "Source vertex not found during recovery: label={}, vid={:?}",
                                    redo.src_label, redo.src_vid
                                ))
                            },
                        )?;
                    let dst_internal =
                        TransactionOps::resolve_vertex_id(dst_table, redo.dst_vid, ts).ok_or_else(
                            || {
                                StorageError::db_error(format!(
                            "Destination vertex not found during recovery: label={}, vid={:?}",
                            redo.dst_label, redo.dst_vid
                        ))
                            },
                        )?;
                    Ok((src_internal, dst_internal))
                })?;

        let params = AddEdgeParams {
            src_label: redo.src_label,
            src_vid: src_internal,
            dst_label: redo.dst_label,
            dst_vid: dst_internal,
            edge_label: redo.edge_label,
            rank: redo.rank,
        };

        {
            let mut catalog = self.data_store().catalog_write_set();

            if let Err(e) = TransactionOps::add_edge(
                &mut catalog.edge_tables,
                &catalog.vertex_tables,
                params,
                &redo.properties,
                ts,
            ) {
                if !is_replayable_edge_duplicate(&e) {
                    return Err(StorageError::db_error(format!(
                        "Failed to replay insert edge: {}",
                        e
                    )));
                }
            }
        }

        self.mark_edge_modified(redo.edge_label);
        Ok(())
    }

    pub(crate) fn do_replay_delete_edge(
        &self,
        redo: &DeleteEdgeRedo,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let key = crate::engine::data_store::EdgeTableKey::new(
            redo.src_label,
            redo.dst_label,
            redo.edge_label,
        );

        let (src_internal, dst_internal) =
            self.data_store()
                .with_vertex_tables(|vertex_tables| -> StorageResult<(u32, u32)> {
                    let src_internal =
                        resolve_external_vid(vertex_tables, redo.src_label, redo.src_vid, ts)
                            .ok_or_else(|| {
                                StorageError::db_error(format!(
                        "Source vertex not found during delete-edge recovery: label={}, vid={:?}",
                        redo.src_label, redo.src_vid
                    ))
                            })?;
                    let dst_internal =
                        resolve_external_vid(vertex_tables, redo.dst_label, redo.dst_vid, ts)
                            .ok_or_else(|| {
                                StorageError::db_error(format!(
                    "Destination vertex not found during delete-edge recovery: label={}, vid={:?}",
                    redo.dst_label, redo.dst_vid
                ))
                            })?;
                    Ok((src_internal, dst_internal))
                })?;

        let arc = self
            .data_store()
            .with_edge_tables(|tables| tables.get(&key).cloned());
        if let Some(arc) = arc {
            let mut table = arc.write();
            let _ = table.delete_edge(src_internal, dst_internal, redo.rank, ts)?;
        }

        self.mark_edge_modified(redo.edge_label);
        Ok(())
    }

    pub(crate) fn replay_vertex_index_update(
        &self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        let Some((space_name, tag_info)) = self.schema_manager().find_tag_by_id(label) else {
            return Ok(());
        };
        let Some(space_info) = self.schema_manager().get_space(&space_name)? else {
            return Ok(());
        };
        let space_id = space_info.space_id;
        if properties.is_empty() {
            return Ok(());
        }
        let indexes = self.index_metadata_manager().list_tag_indexes(space_id)?;
        let vid_value = Value::from(vid);
        for index in indexes {
            if index.schema_name == tag_info.tag_name {
                self.update_vertex_indexes_mvcc(space_id, &vid_value, &index.name, properties, ts)?;
            }
        }
        Ok(())
    }

    pub(crate) fn replay_vertex_index_delete(
        &self,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        let Some((space_name, tag_info)) = self.schema_manager().find_tag_by_id(label) else {
            return Ok(());
        };
        let Some(space_info) = self.schema_manager().get_space(&space_name)? else {
            return Ok(());
        };
        let space_id = space_info.space_id;
        let index_names: Vec<String> = self
            .index_metadata_manager()
            .list_tag_indexes(space_id)?
            .into_iter()
            .filter(|index| index.schema_name == tag_info.tag_name)
            .map(|index| index.name)
            .collect();
        if !index_names.is_empty() {
            let vid_value = Value::from(vid);
            self.delete_vertex_indexes_mvcc(space_id, &vid_value, &index_names, ts)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replayable_duplicate_accepts_both_spellings() {
        let spaced = UndoLogError::UndoFailed("Resource already exists: 0 -> 1".to_string());
        let underscored = UndoLogError::UndoFailed("[edge_already_exists] 0 -> 1@0".to_string());
        assert!(is_replayable_edge_duplicate(&spaced));
        assert!(is_replayable_edge_duplicate(&underscored));
        let conflict = UndoLogError::UndoFailed("Write-write conflict: e".to_string());
        assert!(!is_replayable_edge_duplicate(&conflict));
    }

    #[test]
    fn benign_replay_missing_covers_absent_targets_only() {
        assert!(is_benign_replay_missing(&UndoLogError::LabelNotFound(1)));
        assert!(is_benign_replay_missing(&UndoLogError::EdgeNotFound(
            graphdb_core::types::EdgeId(7)
        )));
        assert!(is_benign_replay_missing(&UndoLogError::PropertyNotFound(
            "weight".to_string()
        )));
        assert!(is_benign_replay_missing(&UndoLogError::UndoFailed(
            "column not found: score".to_string()
        )));
        assert!(is_benign_replay_missing(&UndoLogError::UndoFailed(
            "Edge does not exist: 0 -> 1".to_string()
        )));
        assert!(!is_benign_replay_missing(&UndoLogError::UndoFailed(
            "Write-write conflict: edge already deleted at ts=5".to_string()
        )));
        assert!(!is_benign_replay_missing(&UndoLogError::UndoFailed(
            "type mismatch: expected Int".to_string()
        )));
    }
}

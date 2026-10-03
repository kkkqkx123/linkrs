use std::collections::HashMap;

use graphdb_core::types::{Timestamp, VertexId};
use graphdb_core::wal::redo::{DeleteVertexPropsRedo, InsertVertexRedo, UpdateVertexPropRedo};
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{DataType, StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::wal::TransactionWalEntry;
use graphdb_transaction::{MutationEntityKey, MutationResult};

use super::super::context::txn_staging::StagedIndexOp;
use super::super::context::GraphStorageContext;
use super::super::ops::{route_vertex_id, tag_label_id, RoutedVertexId};
use super::vertex_batch::StagedVertexRow;
use crate::vertex::{primary_key_mirror_value, IdKey};

/// Record one vertex insert for the transaction write set. Vertex rollback
/// is the staged buffer's, so no per-row undo entry is generated; the redo
/// entry keeps the WAL replay form.
pub(super) fn record_vertex_insert(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

/// Record one vertex delete for the transaction write set. See
/// [`record_vertex_insert`] for why no undo entry is generated.
pub(super) fn record_vertex_remove(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

/// Record one vertex property update for the transaction write set. See
/// [`record_vertex_insert`] for why no undo entry is generated.
pub(super) fn record_vertex_property_update(
    ctx: &GraphStorageContext,
    vid: VertexId,
    redo_entry: Option<TransactionWalEntry>,
) -> StorageResult<()> {
    let Some(recorder) = ctx.mutation_recorder() else {
        return Ok(());
    };
    recorder
        .record_mutation(MutationResult {
            entity_keys: vec![MutationEntityKey::Vertex(vid)],
            undo_entry: None,
            redo_entry,
            modified_table: Some("vertex".to_string()),
            ..MutationResult::default()
        })
        .map_err(|error| StorageError::db_error(error.to_string()))?;
    Ok(())
}

pub(crate) fn insert_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<VertexId> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    // The vertex type carries exactly one tag by construction, so no
    // multi-tag gate is needed: normalization runs before any timestamp is
    // allocated and a bad id fails the whole write up front.
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;
    let vertex = Vertex::new(vid, vertex.tag);

    let ts = ctx.get_write_timestamp()?;
    // Caller-owned staging scope created with the write timestamp; it
    // buffers the row through context and shard layers without touching
    // global state, the apply step below installs it, and rollback or crash
    // drop discards it.
    let mut scope = crate::vertex::WriteScope::new(ts);
    log::trace!("write scope opened ts={}", scope.write_ts());
    let staged = match stage_vertex_row_single(ctx, space, vertex, ts, &mut scope) {
        Ok(staged) => staged,
        Err(error) => {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    };

    // Online writes hold in the transaction buffer: absorb the staged row,
    // journal the transaction record and the index maintenance, and let the
    // commit point apply everything. Nothing global is touched past this
    // point, so a failure only rolls the buffer back to the statement mark.
    if ctx.is_online_write() {
        if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &staged.vertex_id,
            &staged.tag_name,
            &staged.props,
        ) {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        let (buffer, mark) = match ctx.txn_staging_mark(ts) {
            Ok(mark) => mark,
            Err(error) => {
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        };
        let outcome = ctx
            .absorb_write_scope(&mut scope, ts)
            .and_then(|()| record_vertex_insert(ctx, staged.vid, Some(staged.redo_entry.clone())))
            .and_then(|()| {
                ctx.stage_vertex_index_op(
                    ts,
                    StagedIndexOp::Insert {
                        space_id: space_info.space_id,
                        vid: staged.vertex_id.clone(),
                        tag: staged.tag_name.clone(),
                        properties: staged.props.clone(),
                    },
                )
            });
        if let Err(error) = outcome {
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        return Ok(staged.vid);
    }

    // Apply: the commit hook installs the row and consumes the staging
    // record. A failed apply undoes its own partials inside the hook, the
    // row is not visible yet (the timestamp is still pending), and no index
    // entry exists, so the compensation is a scope discard plus abort.
    let internal_id = match ctx
        .commit_write_scope(staged.label_id, &mut scope, ts)
        .and_then(|mapping| {
            mapping
                .into_iter()
                .find(|(key, _)| *key == staged.key)
                .map(|(_, global_id)| global_id)
                .ok_or_else(|| {
                    StorageError::db_error(format!(
                        "commit apply lost staged vertex {:?}",
                        staged.vid
                    ))
                })
        }) {
        Ok(internal_id) => internal_id,
        Err(error) => {
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    };
    debug_assert!(scope.is_empty());

    // Index maintenance and the mutation record run after the row is
    // installed and before the timestamp publishes visibility. A failure
    // here leaves an applied row behind, so it is compensated by undoing
    // the install (dropping the identity and row slots for this
    // not-yet-visible timestamp) and clearing any index entries written.
    if let Err(error) = super::index_maintenance::update_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &staged.vertex_id,
        &staged.tag_name,
        &staged.props,
        ts,
    ) {
        compensate_inserted_vertex(ctx, space_info.space_id, &staged, internal_id, ts);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }
    if let Err(error) = record_vertex_insert(ctx, staged.vid, Some(staged.redo_entry.clone())) {
        compensate_inserted_vertex(ctx, space_info.space_id, &staged, internal_id, ts);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(error);
    }

    // Cache and domain bookkeeping feed from the commit mapping's id.
    match &staged.key {
        IdKey::Int(vid_int) => {
            ctx.cache_inserted_vertex_id(staged.label_id, &vid_int.to_string(), internal_id, ts);
            ctx.mark_vertex_modified(staged.label_id);
            ctx.observe_vertex_id_i64(staged.label_id, *vid_int);
        }
        IdKey::Text(id_str) => {
            ctx.cache_inserted_vertex_id(staged.label_id, id_str, internal_id, ts);
            ctx.mark_vertex_modified(staged.label_id);
            ctx.observe_vertex_id_string(staged.label_id);
        }
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(staged.vid)
}

/// Undo a post-apply insert failure: clear index entries written for the
/// row, then drop the applied row itself through the scope-apply undo hook.
fn compensate_inserted_vertex(
    ctx: &GraphStorageContext,
    space_id: u64,
    staged: &StagedVertexRow,
    internal_id: u32,
    ts: Timestamp,
) {
    let _ = super::index_maintenance::delete_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_id,
        &staged.vertex_id,
        &staged.tag_name,
        ts,
    );
    ctx.undo_applied_scope_inserts(staged.label_id, &[internal_id]);
}

fn stage_vertex_row_single(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
    ts: Timestamp,
    scope: &mut crate::vertex::WriteScope,
) -> StorageResult<StagedVertexRow> {
    let tag = &vertex.tag;
    let label_id = tag_label_id(ctx, space, &tag.name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
    let props: Vec<(String, Value)> = tag
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let props = super::constraints::apply_tag_constraints(ctx, space, &tag.name, props)?;
    let redo = InsertVertexRedo {
        label: label_id,
        vid: vertex.vid,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;
    let key = match route_vertex_id(&vertex.vid)? {
        RoutedVertexId::Int(vid_int) => IdKey::Int(vid_int),
        RoutedVertexId::Text(id_str) => IdKey::Text(id_str),
    };
    match &key {
        IdKey::Int(vid_int) => {
            ctx.insert_vertex_by_i64_with_scope(label_id, *vid_int, &props, ts, scope)?;
        }
        IdKey::Text(id_str) => {
            ctx.insert_vertex_with_scope(label_id, id_str, &props, ts, scope)?;
        }
    }
    Ok(StagedVertexRow {
        label_id,
        vid: vertex.vid,
        key,
        vertex_id: Value::from(vertex.vid),
        tag_name: tag.name.clone(),
        props,
        redo_entry,
    })
}

pub(crate) fn update_vertex(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let tag = &vertex.tag;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;

    let ts = ctx.get_write_timestamp()?;
    let label_id = tag_label_id(ctx, space, &tag.name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;

    // Online updates buffer in the transaction staging hold; the statement
    // mark lets a mid-statement failure unwind exactly this statement's
    // staged rows with nothing applied.
    let online = ctx.is_online_write();
    let staging = if online {
        match ctx.txn_staging_mark(ts) {
            Ok(mark) => Some(mark),
            Err(error) => {
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        }
    } else {
        None
    };
    let unwind = |ctx: &GraphStorageContext, error: StorageError| -> StorageError {
        if let Some((buffer, mark)) = &staging {
            ctx.rollback_staging_to(buffer, *mark);
        }
        ctx.abort_write_timestamp(ts);
        error
    };

    // Updates merge the full existing row, including the primary
    // key mirror, into the write set. Restating the mirror changes nothing
    // and is skipped; a divergent key is rejected by the update entry
    // (table layer offline, staging validation online).
    let pk_mirror: Option<(String, DataType, Value)> =
        ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&label_id).and_then(|table| {
                let schema = table.schema();
                let pk = schema.properties.get(schema.primary_key_index)?;
                let key = match route_vertex_id(&vid).ok()? {
                    RoutedVertexId::Int(id) => IdKey::Int(id),
                    RoutedVertexId::Text(id) => IdKey::Text(id),
                };
                primary_key_mirror_value(&pk.data_type, &key)
                    .ok()
                    .map(|mirror| (pk.name.clone(), pk.data_type.clone(), mirror))
            })
        });

    let current_record = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(id_int) => ctx.get_vertex_by_i64(label_id, id_int, ts),
        RoutedVertexId::Text(id_str) => ctx.get_vertex(label_id, &id_str, ts),
    };

    let mut merged_props: HashMap<String, Value> = current_record
        .as_ref()
        .map(|record| record.properties.iter().cloned().collect())
        .unwrap_or_default();
    for (prop_name, value) in &tag.properties {
        merged_props.insert(prop_name.clone(), value.clone());
    }

    for (prop_name, value) in &tag.properties {
        let restates_mirror = match pk_mirror.as_ref() {
            Some((pk_name, pk_type, mirror)) if prop_name == pk_name => {
                value == mirror || value.try_cast_to(pk_type).is_ok_and(|cast| &cast == mirror)
            }
            _ => false,
        };
        if restates_mirror {
            continue;
        }
        let redo = UpdateVertexPropRedo {
            label: label_id,
            vid,
            prop_name: prop_name.clone(),
            value: value.clone(),
        };
        let redo_entry = match ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo) {
            Ok(entry) => entry,
            Err(error) => return Err(unwind(ctx, error)),
        };

        // The table layer rejects primary key columns and missing rows.
        let update_result = match route_vertex_id(&vid)? {
            RoutedVertexId::Int(id_int) => {
                ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)
            }
            RoutedVertexId::Text(id_str) => {
                ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)
            }
        };
        if let Err(error) = update_result {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = record_vertex_property_update(ctx, vid, Some(redo_entry)) {
            return Err(unwind(ctx, error));
        }
    }

    let props: Vec<(String, Value)> = merged_props.into_iter().collect();
    let vid_value = Value::from(vid);
    if online {
        // The refreshed row values must not collide with another vertex's
        // unique entries at the statement; the index mutation itself
        // replays at commit apply, after the new bytes land.
        if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &vid_value,
            &tag.name,
            &props,
        ) {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = ctx.stage_vertex_index_op(
            ts,
            StagedIndexOp::Update {
                space_id: space_info.space_id,
                vid: vid_value,
                tag: tag.name.clone(),
                properties: props,
            },
        ) {
            return Err(unwind(ctx, error));
        }
        return Ok(());
    }

    if let Err(error) = super::index_maintenance::refresh_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &vid_value,
        &tag.name,
        &props,
        ts,
    ) {
        return Err(unwind(ctx, error));
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
}

pub(crate) fn update_vertex_replace(
    ctx: &GraphStorageContext,
    space: &str,
    vertex: Vertex,
) -> StorageResult<()> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let tag = &vertex.tag;
    let vid = VertexId::normalize_for_vid_type(&space_info.vid_type, vertex.vid)?;
    let ts = ctx.get_write_timestamp()?;
    let label_id = tag_label_id(ctx, space, &tag.name)?
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
    let online = ctx.is_online_write();
    let staging = if online {
        match ctx.txn_staging_mark(ts) {
            Ok(mark) => Some(mark),
            Err(error) => {
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        }
    } else {
        None
    };
    let unwind = |ctx: &GraphStorageContext, error: StorageError| -> StorageError {
        if let Some((buffer, mark)) = &staging {
            ctx.rollback_staging_to(buffer, *mark);
        }
        ctx.abort_write_timestamp(ts);
        error
    };
    let pk_mirror: Option<(String, DataType, Value)> =
        ctx.data_store().with_vertex_tables(|tables| {
            tables.get(&label_id).and_then(|table| {
                let schema = table.schema();
                let pk = schema.properties.get(schema.primary_key_index)?;
                let key = match route_vertex_id(&vid).ok()? {
                    RoutedVertexId::Int(id) => IdKey::Int(id),
                    RoutedVertexId::Text(id) => IdKey::Text(id),
                };
                primary_key_mirror_value(&pk.data_type, &key)
                    .ok()
                    .map(|mirror| (pk.name.clone(), pk.data_type.clone(), mirror))
            })
        });
    let pk_name = pk_mirror.as_ref().map(|(name, _, _)| name.clone());
    let current_record = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(id_int) => ctx.get_vertex_by_i64(label_id, id_int, ts),
        RoutedVertexId::Text(id_str) => ctx.get_vertex(label_id, &id_str, ts),
    };
    let current_props: HashMap<String, Value> = current_record
        .as_ref()
        .map(|record| record.properties.iter().cloned().collect())
        .unwrap_or_default();
    if current_record.is_none() {
        return Err(unwind(
            ctx,
            StorageError::not_found(format!("Vertex not found: {}", vid)),
        ));
    }
    let mut new_props: HashMap<String, Value> = HashMap::new();
    for (prop_name, value) in &tag.properties {
        if Some(prop_name) == pk_name.as_ref() {
            continue;
        }
        new_props.insert(prop_name.clone(), value.clone());
    }
    let mut deleted: Vec<String> = current_props
        .keys()
        .filter(|name| !new_props.contains_key(*name))
        .filter(|name| Some(*name) != pk_name.as_ref())
        .cloned()
        .collect();
    deleted.sort();
    for (prop_name, value) in &tag.properties {
        let restates_mirror = match pk_mirror.as_ref() {
            Some((pk_name, pk_type, mirror)) if prop_name == pk_name => {
                value == mirror || value.try_cast_to(pk_type).is_ok_and(|cast| &cast == mirror)
            }
            _ => false,
        };
        if restates_mirror {
            continue;
        }
        if Some(prop_name) == pk_name.as_ref() {
            return Err(unwind(
                ctx,
                StorageError::invalid_operation(format!(
                    "Primary key column '{}' must mirror the vertex id",
                    prop_name
                )),
            ));
        }
        let redo = UpdateVertexPropRedo {
            label: label_id,
            vid,
            prop_name: prop_name.clone(),
            value: value.clone(),
        };
        let redo_entry = match ctx.append_wal_redo(WalOpType::UpdateVertexProp, ts, &redo) {
            Ok(entry) => entry,
            Err(error) => return Err(unwind(ctx, error)),
        };
        let update_result = match route_vertex_id(&vid)? {
            RoutedVertexId::Int(id_int) => {
                ctx.update_vertex_property_by_i64(label_id, id_int, prop_name, value, ts)
            }
            RoutedVertexId::Text(id_str) => {
                ctx.update_vertex_property(label_id, &id_str, prop_name, value, ts)
            }
        };
        if let Err(error) = update_result {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = record_vertex_property_update(ctx, vid, Some(redo_entry)) {
            return Err(unwind(ctx, error));
        }
    }
    if !deleted.is_empty() {
        let redo = DeleteVertexPropsRedo {
            label: label_id,
            vid,
            prop_names: deleted.clone(),
        };
        let redo_entry = match ctx.append_wal_redo(WalOpType::DeleteVertexProps, ts, &redo) {
            Ok(entry) => entry,
            Err(error) => return Err(unwind(ctx, error)),
        };
        for prop_name in &deleted {
            let delete_result = match route_vertex_id(&vid)? {
                RoutedVertexId::Int(id_int) => {
                    ctx.delete_vertex_row_property_by_i64(label_id, id_int, prop_name, ts)
                }
                RoutedVertexId::Text(id_str) => {
                    ctx.delete_vertex_row_property(label_id, &id_str, prop_name, ts)
                }
            };
            if let Err(error) = delete_result {
                return Err(unwind(ctx, error));
            }
        }
        if let Err(error) = record_vertex_property_update(ctx, vid, Some(redo_entry)) {
            return Err(unwind(ctx, error));
        }
    }
    let props: Vec<(String, Value)> = new_props.into_iter().collect();
    let vid_value = Value::from(vid);
    if online {
        if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_info.space_id,
            &vid_value,
            &tag.name,
            &props,
        ) {
            return Err(unwind(ctx, error));
        }
        if let Err(error) = ctx.stage_vertex_index_op(
            ts,
            StagedIndexOp::Update {
                space_id: space_info.space_id,
                vid: vid_value,
                tag: tag.name.clone(),
                properties: props,
            },
        ) {
            return Err(unwind(ctx, error));
        }
        return Ok(());
    }
    if let Err(error) = super::index_maintenance::refresh_vertex_indexes(
        ctx,
        ctx.index_metadata_manager(),
        space_info.space_id,
        &vid_value,
        &tag.name,
        &props,
        ts,
    ) {
        return Err(unwind(ctx, error));
    }
    ctx.commit_write_timestamp_ordered(ts)?;
    Ok(())
}

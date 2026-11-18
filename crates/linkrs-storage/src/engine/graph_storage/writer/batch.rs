use std::collections::{HashMap, HashSet};

use crate::engine::{
    BatchEdgeDelete, BatchEdgeInsert, DeleteEdgesBatchParams, InsertEdgesBatchParams,
};
use crate::index::types::EdgeIdentity;
use crate::types::PropertyRowList;
use linkrs_core::metadata::IndexMetadataManager;
use linkrs_core::types::{
    EdgeIdentifier, EdgeTypeInfo, Index, LabelId, TagInfo, Timestamp, VertexId,
};
use linkrs_core::wal::redo::{DeleteEdgeRedo, InsertEdgeRedo};
use linkrs_core::wal::types::WalOpType;
use linkrs_core::{DataType, Edge, EdgeDeleteKey, StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;
use super::super::ops::endpoint_label_id;

/// Recommended caller-side chunk size for very large edge batches.
///
/// One batch is one atomic unit holding the table lock for the whole apply,
/// so commit latency grows with batch size. Batches up to this size are the
/// measured regime (see `commit_gate_bench`); callers with larger
/// fanouts split into chunks of at most this many edges and treat each chunk
/// as its own atomic unit instead of one giant commit.
pub(crate) const RECOMMENDED_BATCH_CHUNK_EDGES: usize = 10_000;

/// Split an edge batch into caller-side chunks.
///
/// Pure slicing helper behind the chunking contract above: each returned
/// chunk commits atomically on its own, so callers keep the per-commit
/// lock-hold bounded without changing the single-writer discipline. Returns
/// one chunk per `RECOMMENDED_BATCH_CHUNK_EDGES` edges (the tail may be
/// shorter); an empty input yields no chunks.
pub(crate) fn chunk_edges_for_commit(edges: &[Edge]) -> Vec<&[Edge]> {
    if edges.is_empty() {
        return Vec::new();
    }
    edges.chunks(RECOMMENDED_BATCH_CHUNK_EDGES).collect()
}

/// Pre-resolved, per-batch schema context shared by every row of a batch
/// vertex insert: the tag table, the tag indexes, and the SERIAL state all
/// come from a single pass over the batch instead of per-row lookups.
pub(super) struct PrecheckedBatchContext<'a> {
    pub(super) tag_map: &'a HashMap<&'a str, &'a TagInfo>,
    pub(super) tag_indexes: &'a [Index],
    pub(super) serial_state: &'a mut SerialBatchState,
    pub(super) vid_type: &'a DataType,
    /// Scratch serialization buffer reused by every WAL append of the batch,
    /// so postcard writes into stable capacity instead of allocating per row.
    pub(super) wal_scratch: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct InsertedEdgeRecord {
    pub(super) edge_label_id: LabelId,
    pub(super) src_label_id: LabelId,
    pub(super) dst_label_id: LabelId,
    pub(super) src: VertexId,
    pub(super) dst: VertexId,
    pub(super) edge_type: String,
    pub(super) rank: i64,
    /// Staged WAL entry moved into the mutation record on success. Stored
    /// optional so the record loop can take it without cloning the payload;
    /// the rollback path never reads it, only edge identity.
    pub(super) redo_entry: Option<linkrs_transaction::wal::TransactionWalEntry>,
}

/// Edge-type schema resolved once per batch: the definition plus the source
/// and destination label ids every row of that type shares. Batch entries
/// resolve each distinct edge type a single time; row loops reuse the
/// resolved value instead of repeating schema lookups per row.
struct ResolvedEdgeType {
    edge_type: EdgeTypeInfo,
    src_label_id: LabelId,
    dst_label_id: LabelId,
}

/// Batch-local SERIAL validation state: one committed-column snapshot per
/// serial column plus the explicit values already accepted in this batch.
/// Later rows observe earlier batch rows exactly as the per-row rescan did
/// (same-timestamp rows are snapshot-visible), without re-scanning.
pub(super) struct SerialBatchState {
    present: HashMap<(LabelId, String), HashSet<i64>>,
    seen: HashMap<(LabelId, String), HashSet<i64>>,
}

impl SerialBatchState {
    pub(super) fn new() -> Self {
        Self {
            present: HashMap::new(),
            seen: HashMap::new(),
        }
    }

    pub(super) fn add_present(
        &mut self,
        label: LabelId,
        prop_name: &str,
        scan: super::super::serial::SerialColumnScan,
    ) {
        let present: HashSet<i64> = scan.into_present().into_iter().collect();
        self.present.insert((label, prop_name.to_string()), present);
    }

    /// Validate an explicit SERIAL value against committed data and earlier
    /// batch rows, recording it for later rows on success.
    pub(super) fn check_explicit(
        &mut self,
        ctx: &GraphStorageContext,
        space_id: u64,
        table_name: &str,
        label: LabelId,
        prop_name: &str,
        value: &Value,
    ) -> StorageResult<()> {
        let Some(integer) = super::constraints::serial_value_as_i64(value) else {
            return Ok(());
        };
        if integer < 0 {
            return Ok(());
        }
        let key = (label, prop_name.to_string());
        let duplicate = self
            .present
            .get(&key)
            .is_some_and(|present| present.contains(&integer))
            || self
                .seen
                .get(&key)
                .is_some_and(|seen| seen.contains(&integer));
        if duplicate {
            return Err(StorageError::invalid_operation(format!(
                "Duplicate value {} for SERIAL column '{}': the value is already allocated",
                integer, prop_name
            )));
        }
        self.seen.entry(key).or_default().insert(integer);
        ctx.serial_allocator().advance_to(
            &super::super::serial::SerialKey::new(space_id, table_name),
            integer as u64,
        );
        Ok(())
    }
}

pub(crate) fn batch_insert_edges(
    ctx: &GraphStorageContext,
    space: &str,
    edges: Vec<Edge>,
) -> StorageResult<()> {
    if edges.len() > RECOMMENDED_BATCH_CHUNK_EDGES {
        log::debug!(
            "batch_insert_edges holds one atomic commit for {} edges (recommended chunk <= {}); very large fanouts should chunk explicitly via GraphStorage::chunk_edges_for_commit with one chunk per atomic commit",
            edges.len(),
            RECOMMENDED_BATCH_CHUNK_EDGES,
        );
    }
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    let resolved = resolve_edge_batch_types(ctx, space, &edges)?;

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();
    if let Err(e) = batch_insert_grouped(
        ctx,
        space,
        space_info.space_id,
        &edges,
        &resolved,
        ts,
        &mut rollback,
    ) {
        super::edge::rollback_edges(ctx, space_info.space_id, &rollback, ts);
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    for item in rollback.iter_mut() {
        if let Err(e) = super::edge::record_edge_insert(
            ctx,
            EdgeIdentifier::new(
                item.src_label_id,
                item.src,
                item.dst_label_id,
                item.dst,
                item.edge_label_id,
                item.rank,
            ),
            item.redo_entry.take(),
        ) {
            super::edge::rollback_edges(ctx, space_info.space_id, &rollback, ts);
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
    }

    // Explicit transactions settle once at their commit point.
    if ctx.is_online_write() {
        return Ok(());
    }
    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
}

/// Insert a batch grouped by edge type in first-seen order.
///
/// Each type's schema comes from the batch pre-resolution, then the type
/// appends one redo per edge in group order, commits the whole type through
/// a single staging batch, then runs the per-edge index updates in the same
/// order. A failure rolls back every previously applied edge through the
/// shared rollback path, matching the visible-state effects of the
/// sequential loop this replaces.
fn batch_insert_grouped(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    edges: &[Edge],
    resolved: &HashMap<String, ResolvedEdgeType>,
    ts: Timestamp,
    rollback: &mut Vec<InsertedEdgeRecord>,
) -> StorageResult<()> {
    let mut order: Vec<String> = Vec::new();
    let mut by_type: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, edge) in edges.iter().enumerate() {
        by_type
            .entry(edge.edge_type.clone())
            .or_insert_with(|| {
                order.push(edge.edge_type.clone());
                Vec::new()
            })
            .push(index);
    }
    for edge_type_name in &order {
        let positions = by_type
            .get(edge_type_name)
            .ok_or_else(|| StorageError::db_error("batch type group missing".to_string()))?;
        let schema = resolved
            .get(edge_type_name)
            .ok_or_else(|| StorageError::db_error("batch resolved type missing".to_string()))?;
        insert_one_type_batch(ctx, space_id, edges, positions, schema, ts, rollback)?;
    }
    Ok(())
}

fn insert_one_type_batch(
    ctx: &GraphStorageContext,
    space_id: u64,
    edges: &[Edge],
    positions: &[usize],
    schema: &ResolvedEdgeType,
    ts: Timestamp,
    rollback: &mut Vec<InsertedEdgeRecord>,
) -> StorageResult<()> {
    let edge_type = &schema.edge_type;
    let edge_label_id = edge_type.edge_type_id;
    let src_label_id = schema.src_label_id;
    let dst_label_id = schema.dst_label_id;

    // Index names matching this type, listed once per batch: rows below
    // only touch these indexes, and skip index work entirely when empty.
    // The group key (every row's own type string) drives the filter, exactly
    // like the per-row listing this replaces.
    let group_type = &edges[positions[0]].edge_type;
    let matching_indexes: Vec<String> = ctx
        .index_metadata_manager()
        .list_edge_indexes(space_id)?
        .into_iter()
        .filter(|index| index.schema_name == *group_type)
        .map(|index| index.name)
        .collect();

    let mut prepared: Vec<(PropertyRowList, usize)> = Vec::with_capacity(positions.len());
    for &index in positions {
        let edge = &edges[index];
        let props: PropertyRowList = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let props = super::constraints::apply_edge_type_constraints_resolved(
            ctx,
            space_id,
            &edge.edge_type,
            edge_type,
            props,
        )?;
        prepared.push((props, index));
    }

    let mut wal_scratch = Vec::new();
    for (props, index) in &prepared {
        let edge = &edges[*index];
        let redo = InsertEdgeRedo {
            src_label: src_label_id,
            src_vid: edge.src,
            dst_label: dst_label_id,
            dst_vid: edge.dst,
            edge_label: edge_label_id,
            rank: edge.ranking,
            properties: props.clone(),
        };
        // WAL first, insert second: on failure nothing is staged yet and
        // the timestamp aborts; the entry moves into the rollback record
        // so the success path below can take it instead of cloning.
        let redo_entry =
            ctx.append_wal_redo_buffered(WalOpType::InsertEdge, ts, &redo, &mut wal_scratch)?;
        rollback.push(InsertedEdgeRecord {
            edge_label_id,
            src_label_id,
            dst_label_id,
            src: edge.src,
            dst: edge.dst,
            edge_type: edge.edge_type.clone(),
            rank: edge.ranking,
            redo_entry: Some(redo_entry),
        });
    }

    let batch_params: Vec<BatchEdgeInsert> = prepared
        .iter()
        .map(|(props, index)| {
            let edge = &edges[*index];
            BatchEdgeInsert {
                src_id: edge.src,
                dst_id: edge.dst,
                rank: edge.ranking,
                properties: props,
            }
        })
        .collect();
    ctx.insert_edges_batch(InsertEdgesBatchParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        dst_label: dst_label_id,
        edges: &batch_params,
        ts,
    })?;

    for (props, index) in &prepared {
        let edge = &edges[*index];
        let src_value = Value::from(edge.src);
        let dst_value = Value::from(edge.dst);
        let edge_identity = EdgeIdentity::new(
            space_id,
            &src_value,
            &dst_value,
            &edge.edge_type,
            edge.ranking,
        );
        for index_name in &matching_indexes {
            ctx.update_edge_indexes_mvcc(&edge_identity, index_name, props, ts)?;
        }
    }

    Ok(())
}

/// Resolve one edge type plus the source and destination label ids its rows
/// share. Missing types or endpoint tags fail exactly like the former
/// per-row validation did.
fn resolve_one_edge_type(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type_name: &str,
) -> StorageResult<ResolvedEdgeType> {
    let edge_type = super::edge::resolve_edge_type(ctx, space, edge_type_name)?;
    let src_label_id =
        endpoint_label_id(ctx, space, &edge_type.src_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!("Source tag {} not found", edge_type.src_tag_name))
        })?;
    let dst_label_id =
        endpoint_label_id(ctx, space, &edge_type.dst_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!(
                "Destination tag {} not found",
                edge_type.dst_tag_name
            ))
        })?;
    Ok(ResolvedEdgeType {
        edge_type,
        src_label_id,
        dst_label_id,
    })
}

/// Resolve every distinct edge type of a batch exactly once: the type
/// definition plus the source and destination label ids its rows share.
/// Runs before the write timestamp is taken, so missing types or endpoint
/// tags fail the batch the same way the former per-row validation did.
fn resolve_edge_batch_types(
    ctx: &GraphStorageContext,
    space: &str,
    edges: &[Edge],
) -> StorageResult<HashMap<String, ResolvedEdgeType>> {
    let mut resolved: HashMap<String, ResolvedEdgeType> = HashMap::new();
    for edge in edges {
        if resolved.contains_key(&edge.edge_type) {
            continue;
        }
        let schema = resolve_one_edge_type(ctx, space, &edge.edge_type)?;
        resolved.insert(edge.edge_type.clone(), schema);
    }
    Ok(resolved)
}

/// Delete a batch of edges by key under one write timestamp.
///
/// Mirrors `batch_insert_edges`: one timestamp covers the whole batch, each
/// edge type resolves its schema once, one redo per key precedes one staging
/// commit per owner partition, then per-edge index cleanup and undo records
/// run for the keys that prefetched live state. Keys never created delete
/// nothing and consume no tombstone; re-deleting an already-deleted edge
/// fails the batch exactly like the single delete. A failure aborts
/// the timestamp with prior groups already committed; aborted stamps stay
/// hidden through the pending gate, matching the vertex-batch contract.
/// Returns the number of net applied deletes.
pub(crate) fn batch_delete_edges(
    ctx: &GraphStorageContext,
    space: &str,
    deletes: &[EdgeDeleteKey],
) -> StorageResult<usize> {
    if deletes.is_empty() {
        return Ok(0);
    }
    let space_id = ctx.schema_manager().get_space_id(space)?;
    // Distinct types validate once up front; the per-key prefetch below
    // still reads each key's previous state for the undo records.
    let mut seen_delete_types: HashSet<&str> = HashSet::new();
    for key in deletes {
        if !seen_delete_types.insert(key.edge_type.as_str()) {
            continue;
        }
        resolve_one_edge_type(ctx, space, &key.edge_type)?;
    }
    // Prefetch previous properties for the undo records, mirroring the
    // single-delete pre-delete read. Missing edges prefetch to `None` and
    // are skipped below with no further effects.
    let mut previous = Vec::with_capacity(deletes.len());
    for key in deletes {
        previous.push(super::super::reader::get_edge(
            ctx,
            space,
            &key.src,
            &key.dst,
            &key.edge_type,
            key.ranking,
        )?);
    }

    let ts = ctx.get_write_timestamp()?;
    let mut deleted = 0usize;
    if let Err(e) = batch_delete_grouped(ctx, space, space_id, deletes, &previous, ts, &mut deleted)
    {
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    // Explicit transactions settle once at their commit point.
    if ctx.is_online_write() {
        return Ok(deleted);
    }
    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(deleted)
}

/// Delete a batch grouped by edge type in first-seen order.
///
/// Each type resolves its schema once, appends one redo per existing edge in
/// group order, commits the whole type through one staging commit per owner
/// partition, then runs the per-edge index cleanup and undo recording in the
/// same order.
fn batch_delete_grouped(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    deletes: &[EdgeDeleteKey],
    previous: &[Option<Edge>],
    ts: Timestamp,
    deleted: &mut usize,
) -> StorageResult<()> {
    let mut order: Vec<String> = Vec::new();
    let mut by_type: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, key) in deletes.iter().enumerate() {
        by_type
            .entry(key.edge_type.clone())
            .or_insert_with(|| {
                order.push(key.edge_type.clone());
                Vec::new()
            })
            .push(index);
    }
    for edge_type_name in &order {
        let positions = by_type
            .get(edge_type_name)
            .ok_or_else(|| StorageError::db_error("batch type group missing".to_string()))?;
        *deleted += delete_one_type_batch(ctx, space, space_id, deletes, positions, previous, ts)?;
    }
    Ok(())
}

fn delete_one_type_batch(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    deletes: &[EdgeDeleteKey],
    positions: &[usize],
    previous: &[Option<Edge>],
    ts: Timestamp,
) -> StorageResult<usize> {
    let first = &deletes[positions[0]];
    let edge_type = super::edge::resolve_edge_type(ctx, space, &first.edge_type)?;
    let edge_label_id = edge_type.edge_type_id;
    let src_label_id =
        endpoint_label_id(ctx, space, &edge_type.src_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!("Source tag {} not found", edge_type.src_tag_name))
        })?;
    let dst_label_id =
        endpoint_label_id(ctx, space, &edge_type.dst_tag_name)?.ok_or_else(|| {
            StorageError::not_found(format!(
                "Destination tag {} not found",
                edge_type.dst_tag_name
            ))
        })?;

    let mut redo_entries = Vec::with_capacity(positions.len());
    // Redo precedes the table commit for every key, mirroring the single
    // delete: replay of a delete matching nothing is a no-op, while
    // re-deleting an already-deleted edge fails in the table batch below
    // exactly like the single path instead of reporting silent success.
    for &index in positions {
        let key = &deletes[index];
        let redo = DeleteEdgeRedo {
            src_label: src_label_id,
            src_vid: key.src,
            dst_label: dst_label_id,
            dst_vid: key.dst,
            edge_label: edge_label_id,
            rank: key.ranking,
        };
        redo_entries.push(ctx.append_wal_redo(WalOpType::DeleteEdge, ts, &redo)?);
    }

    let batch_params: Vec<BatchEdgeDelete> = positions
        .iter()
        .map(|&index| {
            let key = &deletes[index];
            BatchEdgeDelete {
                src_id: key.src,
                dst_id: key.dst,
                rank: key.ranking,
            }
        })
        .collect();
    let applied = ctx.delete_edges_batch(DeleteEdgesBatchParams {
        edge_label: edge_label_id,
        src_label: src_label_id,
        dst_label: dst_label_id,
        edges: &batch_params,
        ts,
    })?;

    for (position, &index) in positions.iter().enumerate() {
        if previous[index].is_none() {
            continue;
        }
        let key = &deletes[index];
        let src_value = Value::from(key.src);
        let dst_value = Value::from(key.dst);
        let edge_identity = EdgeIdentity::new(
            space_id,
            &src_value,
            &dst_value,
            &key.edge_type,
            key.ranking,
        );
        ctx.delete_all_edge_indexes_mvcc(&edge_identity, ts)?;
        let props = previous[index]
            .as_ref()
            .map(|edge| {
                edge.props
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();
        super::edge::record_edge_remove(
            ctx,
            EdgeIdentifier::new(
                src_label_id,
                key.src,
                dst_label_id,
                key.dst,
                edge_label_id,
                key.ranking,
            ),
            props,
            Some(redo_entries[position].clone()),
        )?;
    }

    Ok(applied)
}

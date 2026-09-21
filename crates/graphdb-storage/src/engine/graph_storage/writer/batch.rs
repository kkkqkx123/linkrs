use std::collections::{HashMap, HashSet};

use crate::engine::{BatchEdgeInsert, InsertEdgesBatchParams};
use crate::index::types::EdgeIdentity;
use graphdb_core::types::{EdgeIdentifier, Index, LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::InsertEdgeRedo;
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{Edge, StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;
use super::super::ops::endpoint_label_id;

/// Pre-resolved, per-batch schema context shared by every row of a batch
/// vertex insert: the tag table, the tag indexes, and the SERIAL state all
/// come from a single pass over the batch instead of per-row lookups.
pub(super) struct PrecheckedBatchContext<'a> {
    pub(super) tag_map: &'a HashMap<&'a str, &'a TagInfo>,
    pub(super) tag_indexes: &'a [Index],
    pub(super) serial_state: &'a mut SerialBatchState,
}

#[derive(Debug)]
pub(super) struct InsertedVertexTag {
    pub(super) label_id: LabelId,
    pub(super) id: String,
    pub(super) vid: VertexId,
    pub(super) vertex_id: Value,
    pub(super) tag_name: String,
    pub(super) redo_entry: graphdb_transaction::wal::TransactionWalEntry,
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
    pub(super) redo_entry: graphdb_transaction::wal::TransactionWalEntry,
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
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    validate_edge_batch(ctx, space, &edges)?;

    let ts = ctx.get_write_timestamp()?;
    let mut rollback = Vec::new();
    if let Err(e) = batch_insert_grouped(ctx, space, space_info.space_id, &edges, ts, &mut rollback)
    {
        super::edge::rollback_edges(ctx, space_info.space_id, &rollback, ts);
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    for item in &rollback {
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
            Some(item.redo_entry.clone()),
        ) {
            super::edge::rollback_edges(ctx, space_info.space_id, &rollback, ts);
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
    }

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(())
}

/// Insert a batch grouped by edge type in first-seen order.
///
/// Each type resolves its schema once, appends one redo per edge in group
/// order, commits the whole type through a single staging batch, then runs
/// the per-edge index updates in the same order. A failure rolls back every
/// previously applied edge through the shared rollback path, matching the
/// visible-state effects of the sequential loop this replaces.
fn batch_insert_grouped(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    edges: &[Edge],
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
        insert_one_type_batch(ctx, space, space_id, edges, positions, ts, rollback)?;
    }
    Ok(())
}

fn insert_one_type_batch(
    ctx: &GraphStorageContext,
    space: &str,
    space_id: u64,
    edges: &[Edge],
    positions: &[usize],
    ts: Timestamp,
    rollback: &mut Vec<InsertedEdgeRecord>,
) -> StorageResult<()> {
    let first = &edges[positions[0]];
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

    let mut prepared: Vec<(Vec<(String, Value)>, usize)> = Vec::with_capacity(positions.len());
    for &index in positions {
        let edge = &edges[index];
        let props: Vec<(String, Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let props =
            super::constraints::apply_edge_type_constraints(ctx, space, &edge.edge_type, props)?;
        prepared.push((props, index));
    }

    let mut redo_entries = Vec::with_capacity(prepared.len());
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
        redo_entries.push(ctx.append_wal_redo(WalOpType::InsertEdge, ts, &redo)?);
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

    for (position, index) in prepared.iter().map(|(_, index)| index).enumerate() {
        let edge = &edges[*index];
        rollback.push(InsertedEdgeRecord {
            edge_label_id,
            src_label_id,
            dst_label_id,
            src: edge.src,
            dst: edge.dst,
            edge_type: edge.edge_type.clone(),
            rank: edge.ranking,
            redo_entry: redo_entries[position].clone(),
        });
    }

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
        ctx.update_all_edge_indexes_mvcc(&edge_identity, props, ts)?;
    }

    Ok(())
}

fn validate_edge_batch(
    ctx: &GraphStorageContext,
    space: &str,
    edges: &[Edge],
) -> StorageResult<()> {
    for edge in edges {
        let edge_type = super::edge::resolve_edge_type(ctx, space, &edge.edge_type)?;
        if endpoint_label_id(ctx, space, &edge_type.src_tag_name)?.is_none() {
            return Err(StorageError::not_found(format!(
                "Source tag {} not found",
                edge_type.src_tag_name
            )));
        }
        if endpoint_label_id(ctx, space, &edge_type.dst_tag_name)?.is_none() {
            return Err(StorageError::not_found(format!(
                "Destination tag {} not found",
                edge_type.dst_tag_name
            )));
        }
    }
    Ok(())
}

use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::{LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::wal::redo::InsertVertexRedo;
use graphdb_core::wal::types::WalOpType;
use graphdb_core::{StorageError, StorageResult, Value, Vertex};
use graphdb_transaction::wal::TransactionWalEntry;

use super::super::context::txn_staging::StagedIndexOp;
use super::super::context::GraphStorageContext;
use super::super::ops::{route_vertex_id, RoutedVertexId};
use super::super::serial::scan_vertex_serial_column;
use super::batch::{PrecheckedBatchContext, SerialBatchState};
use crate::vertex::IdKey;

use super::vertex::record_vertex_insert;

/// Undo every row installed by the batch apply phase, label by label.
fn undo_applied_batch(ctx: &GraphStorageContext, applied: &[(LabelId, Vec<u32>)]) {
    for (label_id, ids) in applied {
        ctx.undo_applied_scope_inserts(*label_id, ids);
    }
}

/// Best-effort removal of every secondary index entry a batch insert could
/// have written. The batch vids are new, so entries of rows that never
/// reached the index phase are absent and the delete is a no-op.
fn clear_batch_vertex_indexes(
    ctx: &GraphStorageContext,
    space_id: u64,
    staged: &[StagedVertexRow],
    ts: Timestamp,
) {
    for row in staged {
        let _ = super::index_maintenance::delete_vertex_indexes(
            ctx,
            ctx.index_metadata_manager(),
            space_id,
            &row.vertex_id,
            &row.tag_name,
            ts,
        );
    }
}

/// One validated, WAL-appended vertex insert awaiting table application.
///
/// Phase A of [`batch_insert_vertices`] stages rows in memory; phase B merges
/// them into the tables with shard-grouped writes; phase C finishes indexes
/// and caches. A staged row touches no table or index state, so dropping it
/// is a free abort.
pub(super) struct StagedVertexRow {
    pub(super) label_id: LabelId,
    pub(super) vid: VertexId,
    pub(super) key: IdKey,
    pub(super) vertex_id: Value,
    pub(super) tag_name: String,
    pub(super) props: Vec<(Arc<str>, Value)>,
    pub(super) redo_entry: TransactionWalEntry,
}

/// Validate, constrain, and WAL-append one batch row without touching table
/// or index state. Uses the batch's pre-resolved schema context, so row
/// semantics match the former per-row path.
fn stage_vertex_row(
    ctx: &GraphStorageContext,
    space_id: u64,
    batch: &mut PrecheckedBatchContext<'_>,
    vertex: &Vertex,
    ts: Timestamp,
) -> StorageResult<StagedVertexRow> {
    let tag = &vertex.tag;
    let vid = VertexId::normalize_for_vid_type(batch.vid_type, vertex.vid)?;
    let tag_info = batch
        .tag_map
        .get(tag.name.as_str())
        .ok_or_else(|| StorageError::not_found(format!("Tag {} not found", tag.name)))?;
    let label_id = tag_info.tag_id;
    let props: Vec<(Arc<str>, Value)> = tag
        .properties
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let props = super::constraints::apply_tag_constraints_prechecked(
        ctx,
        space_id,
        tag_info,
        batch.serial_state,
        props,
    )?;
    let redo = InsertVertexRedo {
        label: label_id,
        vid,
        properties: props.clone(),
    };
    let redo_entry = ctx.append_wal_redo(WalOpType::InsertVertex, ts, &redo)?;
    let key = match route_vertex_id(&vid)? {
        RoutedVertexId::Int(vid_int) => IdKey::Int(vid_int),
        RoutedVertexId::Text(id_str) => IdKey::Text(id_str),
    };
    Ok(StagedVertexRow {
        label_id,
        vid,
        key,
        vertex_id: Value::from(vid),
        tag_name: tag.name.clone(),
        props,
        redo_entry,
    })
}

/// Prefix-commit error for auto-batched inserts: `committed` rows of
/// `total` are durable in earlier chunks with independent timestamps; the
/// caller resumes after `committed` or rolls the prefix back through the
/// batch delete path. The single-batch limit stays as backpressure: chunks
/// never exceed [`crate::vertex::MAX_WRITE_SCOPE_KEYS`] rows.
pub(crate) fn batch_prefix_error(
    committed: usize,
    total: usize,
    cause: &StorageError,
) -> StorageError {
    StorageError::db_error(format!(
        "batch prefix committed {}/{} at chunk boundary: {}",
        committed, total, cause
    ))
}

/// Parse [`batch_prefix_error`] back into `(committed, total)`. `None`
/// means the error is not a prefix commit (single-batch failure).
pub(crate) fn batch_prefix_committed(error: &StorageError) -> Option<(usize, usize)> {
    let message = error.message();
    let rest = message.strip_prefix("batch prefix committed ")?;
    let (counts, _) = rest.split_once(" at chunk boundary: ")?;
    let (committed, total) = counts.split_once('/')?;
    Some((committed.parse().ok()?, total.parse().ok()?))
}

pub(crate) fn batch_insert_vertices(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
) -> StorageResult<Vec<VertexId>> {
    batch_insert_vertices_with_split(ctx, space, vertices, true)
}

/// Batch insert with an explicit split switch.
///
/// When `auto_split` is enabled, over-limit inputs loop by label in
/// single-request chunks instead of failing: online writes share one
/// timestamp across chunks while offline writes commit one timestamp per
/// chunk. When disabled, over-limit inputs fail with a capacity error that
/// names the limit and points at this split entry.
pub(crate) fn batch_insert_vertices_with_split(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
    auto_split: bool,
) -> StorageResult<Vec<VertexId>> {
    if vertices.len() > crate::vertex::MAX_WRITE_SCOPE_KEYS && !auto_split {
        return Err(over_limit_split_error(vertices.len()));
    }
    // The shared body below re-checks the limit after tag resolution so
    // the chunked entries stay on the same path.
    batch_insert_vertices_body(ctx, space, vertices)
}

fn over_limit_split_error(total: usize) -> StorageError {
    StorageError::new(
        StorageErrorKind::CapacityExceeded,
        format!(
            "batch holds {} rows above the single-request limit {}: enable split batching by label into smaller chunks instead of growing one request",
            total,
            crate::vertex::MAX_WRITE_SCOPE_KEYS,
        ),
    )
}

fn batch_insert_vertices_body(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
) -> StorageResult<Vec<VertexId>> {
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;

    // Resolve tags once per batch instead of once per row per use site
    // (validation, reserve counting, and insertion each re-looked them up).
    let tags = ctx.schema_manager().list_tags(space)?;
    let mut tag_map: HashMap<&str, &TagInfo> = HashMap::with_capacity(tags.len());
    for tag in &tags {
        tag_map.insert(tag.tag_name.as_str(), tag);
    }
    for vertex in &vertices {
        if !tag_map.contains_key(vertex.tag.name.as_str()) {
            return Err(StorageError::not_found(format!(
                "Tag {} not found",
                vertex.tag.name
            )));
        }
    }

    // Over-limit batches auto-split when enabled instead of rejecting:
    // the single-batch limit stays as backpressure, but the entry chunks
    // the input by label. Online writes share one timestamp across chunks
    // (transaction atomicity); offline writes commit one timestamp per
    // chunk (prefix commits).
    if vertices.len() > crate::vertex::MAX_WRITE_SCOPE_KEYS {
        if ctx.is_online_write() {
            return batch_insert_vertices_online_chunked(ctx, space, vertices);
        }
        return batch_insert_vertices_offline_chunked(ctx, space, vertices);
    }

    // Pre-count vertices per label and reserve capacity to avoid rehashing
    // during inserts. Each vertex carries exactly one label; the map only
    // batches capacity reservations, it is not multi-label bookkeeping.
    {
        let mut per_label_reserve_counts: HashMap<LabelId, usize> = HashMap::new();
        for vertex in &vertices {
            if let Some(info) = tag_map.get(vertex.tag.name.as_str()) {
                *per_label_reserve_counts.entry(info.tag_id).or_insert(0) += 1;
            }
        }
        for (label_id, count) in &per_label_reserve_counts {
            ctx.reserve_vertex_capacity(*label_id, *count);
        }
    }

    // One serial-column scan per touched column for the whole batch. The
    // per-row path scanned the full column for every explicit SERIAL value
    // (O(n log n) per row, O(n^2 log n) per batch); the batch path checks
    // explicit values against this snapshot plus the batch-local seen sets.
    let mut serial_state = SerialBatchState::new();
    for tag in tags.iter() {
        for prop_def in tag.properties.iter().filter(|p| p.serial) {
            let needs_scan = vertices.iter().any(|v| {
                v.tag.name == tag.tag_name && v.tag.properties.keys().any(|k| &**k == prop_def.name)
            });
            if needs_scan {
                if let Some(scan) = scan_vertex_serial_column(ctx, tag.tag_id, &prop_def.name)? {
                    serial_state.add_present(tag.tag_id, &prop_def.name, scan);
                }
            }
        }
    }

    // Fetch tag indexes once per batch instead of once per row.
    let tag_indexes = ctx
        .index_metadata_manager()
        .list_tag_indexes(space_info.space_id)?;

    let ts = ctx.get_write_timestamp()?;
    // Batch write scope created with the batch timestamp; every merge below
    // records into it, and the commit/rollback hooks destroy it. New write
    // entries must wire both hooks (review gate).
    let mut scope = crate::vertex::WriteScope::new(ts);
    log::trace!("batch write scope opened ts={}", scope.write_ts());
    let mut batch_ctx = PrecheckedBatchContext {
        tag_map: &tag_map,
        tag_indexes: &tag_indexes,
        serial_state: &mut serial_state,
        vid_type: &space_info.vid_type,
    };

    // Phase A (stage): validate, constrain, and WAL-append every row in
    // input order. No table or index state is touched, so a failure here
    // only aborts the timestamp; there is nothing to roll back.
    let mut staged: Vec<StagedVertexRow> = Vec::with_capacity(vertices.len());
    for vertex in &vertices {
        match stage_vertex_row(ctx, space_info.space_id, &mut batch_ctx, vertex, ts) {
            Ok(row) => staged.push(row),
            Err(e) => {
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        }
    }

    // Scope capacity is enforced before any global mutation so no applied
    // row ever escapes scope ownership.
    if staged.len() > crate::vertex::MAX_WRITE_SCOPE_KEYS {
        ctx.abort_write_timestamp(ts);
        return Err(over_limit_split_error(staged.len()));
    }

    // Phase B (stage): group staged rows by label and buffer each table's
    // rows without touching global state. Results stay aligned with the
    // input; every row is attempted so the caller can discard every staged
    // row when any row fails. The commit hook below applies the staged rows
    // of every label.
    let mut label_order: Vec<LabelId> = Vec::new();
    let mut by_label: HashMap<LabelId, Vec<usize>> = HashMap::new();
    for (pos, row) in staged.iter().enumerate() {
        by_label
            .entry(row.label_id)
            .or_insert_with(|| {
                label_order.push(row.label_id);
                Vec::new()
            })
            .push(pos);
    }
    let tables: Vec<(LabelId, std::sync::Arc<crate::vertex::ShardedVertexTable>)> =
        match ctx.data_store().with_vertex_tables(|tables| {
            label_order
                .iter()
                .map(|label_id| {
                    tables
                        .get(label_id)
                        .cloned()
                        .map(|t| (*label_id, t))
                        .ok_or_else(|| {
                            StorageError::label_not_found(format!("vertex label {}", label_id))
                        })
                })
                .collect::<StorageResult<Vec<_>>>()
        }) {
            Ok(tables) => tables,
            Err(e) => {
                ctx.abort_write_timestamp(ts);
                return Err(e);
            }
        };
    // `staged_marks[pos]` records whether the row was buffered: staging
    // touches no table state, so a failure here only aborts the timestamp.
    let mut staged_marks: Vec<Option<StorageResult<()>>> =
        (0..staged.len()).map(|_| None).collect();
    for (label_id, table) in &tables {
        let positions = &by_label[label_id];
        let mut str_order: Vec<usize> = Vec::new();
        let mut i64_order: Vec<usize> = Vec::new();
        for &pos in positions {
            match staged[pos].key {
                IdKey::Text(_) => str_order.push(pos),
                IdKey::Int(_) => i64_order.push(pos),
            }
        }
        if !str_order.is_empty() {
            let rows: Vec<(&str, &[(Arc<str>, Value)])> = str_order
                .iter()
                .map(|&pos| {
                    let IdKey::Text(ref s) = staged[pos].key else {
                        unreachable!("str_order only holds text keys");
                    };
                    (s.as_str(), staged[pos].props.as_slice())
                })
                .collect();
            for (slot, result) in str_order
                .iter()
                .zip(table.insert_batch_str_with_scope(&rows, ts, &mut scope))
            {
                staged_marks[*slot] = Some(result);
            }
        }
        if !i64_order.is_empty() {
            let rows: Vec<(i64, &[(Arc<str>, Value)])> = i64_order
                .iter()
                .map(|&pos| {
                    let IdKey::Int(n) = staged[pos].key else {
                        unreachable!("i64_order only holds int keys");
                    };
                    (n, staged[pos].props.as_slice())
                })
                .collect();
            for (slot, result) in i64_order
                .iter()
                .zip(table.insert_batch_i64_with_scope(&rows, ts, &mut scope))
            {
                staged_marks[*slot] = Some(result);
            }
        }
    }

    // Staging touches no table state, so a row failure only discards the
    // staged rows: no apply happened and no index entry exists yet.
    let mut first_error: Option<StorageError> = None;
    for mark in &mut staged_marks {
        if let Some(Err(e)) = mark.take() {
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    if let Some(e) = first_error {
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }

    // Online batch: the whole statement's scope folds into the transaction
    // buffer under one mark; records and index maintenance journal for
    // commit-time replay. A failure unwinds the buffer to the mark and
    // aborts the timestamp with nothing applied.
    if ctx.is_online_write() {
        let (buffer, mark) = match ctx.txn_staging_mark(ts) {
            Ok(mark) => mark,
            Err(error) => {
                scope.clear();
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        };
        let mut outcome = ctx.absorb_write_scope(&mut scope, ts);
        for row in staged.iter() {
            if outcome.is_err() {
                break;
            }
            outcome = super::index_maintenance::check_vertex_unique_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &row.vertex_id,
                &row.tag_name,
                &row.props,
            )
            .and_then(|()| record_vertex_insert(ctx, row.vid, Some(row.redo_entry.clone())))
            .and_then(|()| {
                ctx.stage_vertex_index_op(
                    ts,
                    StagedIndexOp::Insert {
                        space_id: space_info.space_id,
                        vid: row.vertex_id.clone(),
                        tag: row.tag_name.clone(),
                        properties: row.props.clone(),
                    },
                )
            });
        }
        if let Err(error) = outcome {
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        return Ok(staged.iter().map(|row| row.vid).collect());
    }

    // Phase C (apply): commit every label's staged rows. A failed apply
    // undoes its own partial application inside the table hook; labels
    // applied earlier in this loop are tracked here so a mid-batch failure
    // leaves no applied row behind.
    let mut id_by_key: HashMap<(LabelId, IdKey), u32> = HashMap::new();
    let mut applied_by_label: Vec<(LabelId, Vec<u32>)> = Vec::new();
    let mut apply_failure: Option<StorageError> = None;
    for label_id in &label_order {
        match ctx.commit_write_scope(*label_id, &mut scope, ts) {
            Ok(mapping) => {
                let mut ids = Vec::with_capacity(mapping.len());
                for (key, global_id) in mapping {
                    id_by_key.insert((*label_id, key), global_id);
                    ids.push(global_id);
                }
                applied_by_label.push((*label_id, ids));
            }
            Err(e) => {
                apply_failure = Some(e);
                break;
            }
        }
    }
    let lost_vertex = if apply_failure.is_none() {
        staged.iter().find_map(|row| {
            if id_by_key.contains_key(&(row.label_id, row.key.clone())) {
                None
            } else {
                Some(row.vid)
            }
        })
    } else {
        None
    };
    if let Some(vid) = lost_vertex {
        apply_failure = Some(StorageError::db_error(format!(
            "commit apply lost staged vertex {:?}",
            vid
        )));
    }
    if let Some(e) = apply_failure {
        undo_applied_batch(ctx, &applied_by_label);
        scope.clear();
        ctx.abort_write_timestamp(ts);
        return Err(e);
    }
    debug_assert!(scope.is_empty());
    let internal_ids: Vec<u32> = staged
        .iter()
        .map(|row| {
            *id_by_key
                .get(&(row.label_id, row.key.clone()))
                .expect("every staged row is resolved by the apply mapping")
        })
        .collect();

    // Phase D (indexes): per-row index maintenance after the rows are
    // installed and before the timestamp publishes visibility. A failure
    // compensates by clearing every index entry this batch could have
    // written (rows not yet reached are no-ops) and undoing the applied
    // rows.
    for row in staged.iter() {
        if let Err(e) = super::index_maintenance::update_vertex_indexes_with_list(
            ctx,
            batch_ctx.tag_indexes,
            space_info.space_id,
            &row.vertex_id,
            &row.tag_name,
            &row.props,
            ts,
        ) {
            clear_batch_vertex_indexes(ctx, space_info.space_id, &staged, ts);
            undo_applied_batch(ctx, &applied_by_label);
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
    }

    // Phase E: per-row transaction record, then the id-cache and domain
    // bookkeeping fed by the commit mapping.
    for (pos, row) in staged.iter().enumerate() {
        if let Err(e) = record_vertex_insert(ctx, row.vid, Some(row.redo_entry.clone())) {
            clear_batch_vertex_indexes(ctx, space_info.space_id, &staged, ts);
            undo_applied_batch(ctx, &applied_by_label);
            scope.clear();
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
        match &row.key {
            IdKey::Text(id_str) => {
                ctx.cache_inserted_vertex_id(row.label_id, id_str, internal_ids[pos], ts);
                ctx.mark_vertex_modified(row.label_id);
                ctx.observe_vertex_id_string(row.label_id);
            }
            IdKey::Int(vid_int) => {
                ctx.cache_inserted_vertex_id(
                    row.label_id,
                    &vid_int.to_string(),
                    internal_ids[pos],
                    ts,
                );
                ctx.mark_vertex_modified(row.label_id);
                ctx.observe_vertex_id_i64(row.label_id, *vid_int);
            }
        }
    }

    let ids: Vec<VertexId> = staged.iter().map(|row| row.vid).collect();

    ctx.commit_write_timestamp_ordered(ts)?;

    Ok(ids)
}

/// Offline auto-batch: one timestamp per chunk, prefix commits.
/// Each chunk runs the single-batch path (which redoes batch-wide prep per
/// chunk, so later chunks observe earlier prefix commits). A chunk failure
/// keeps earlier prefix commits and returns their count for resume or
/// prefix rollback through the batch delete path. Small-batch semantics
/// are unchanged: chunks never exceed the single-batch limit.
fn batch_insert_vertices_offline_chunked(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
) -> StorageResult<Vec<VertexId>> {
    let total = vertices.len();
    let chunk = crate::vertex::MAX_WRITE_SCOPE_KEYS;
    let mut ids = Vec::with_capacity(total);
    let mut committed = 0usize;
    for window in vertices.chunks(chunk) {
        match batch_insert_vertices(ctx, space, window.to_vec()) {
            Ok(mut chunk_ids) => {
                committed += chunk_ids.len();
                ids.append(&mut chunk_ids);
            }
            Err(cause) => {
                // Nested prefix errors already carry their committed count;
                // add this level's prefix on top.
                if let Some((nested_committed, _)) = batch_prefix_committed(&cause) {
                    committed += nested_committed;
                }
                return Err(batch_prefix_error(committed, total, &cause));
            }
        }
    }
    Ok(ids)
}

/// Online auto-batch: one timestamp for the whole statement, one scope per
/// chunk absorbed sequentially into the transaction buffer. Cross-chunk
/// primary-key duplicates fail like same-scope duplicates (no extra
/// allocation escapes); a failure unwinds the buffer to the statement mark
/// with nothing applied, preserving transaction atomicity.
fn batch_insert_vertices_online_chunked(
    ctx: &GraphStorageContext,
    space: &str,
    vertices: Vec<Vertex>,
) -> StorageResult<Vec<VertexId>> {
    use std::collections::HashSet;
    let space_info = ctx
        .schema_manager()
        .get_space(space)?
        .ok_or_else(|| StorageError::not_found(format!("Space {} not found", space)))?;
    let tags = ctx.schema_manager().list_tags(space)?;
    let mut tag_map: HashMap<&str, &TagInfo> = HashMap::with_capacity(tags.len());
    for tag in &tags {
        tag_map.insert(tag.tag_name.as_str(), tag);
    }
    for vertex in &vertices {
        if !tag_map.contains_key(vertex.tag.name.as_str()) {
            return Err(StorageError::not_found(format!(
                "Tag {} not found",
                vertex.tag.name
            )));
        }
    }
    {
        let mut per_label: HashMap<LabelId, usize> = HashMap::new();
        for vertex in &vertices {
            if let Some(info) = tag_map.get(vertex.tag.name.as_str()) {
                *per_label.entry(info.tag_id).or_insert(0) += 1;
            }
        }
        for (label_id, count) in &per_label {
            ctx.reserve_vertex_capacity(*label_id, *count);
        }
    }
    let mut serial_state = SerialBatchState::new();
    for tag in tags.iter() {
        for prop_def in tag.properties.iter().filter(|p| p.serial) {
            let needs_scan = vertices.iter().any(|v| {
                v.tag.name == tag.tag_name && v.tag.properties.keys().any(|k| &**k == prop_def.name)
            });
            if needs_scan {
                if let Some(scan) = scan_vertex_serial_column(ctx, tag.tag_id, &prop_def.name)? {
                    serial_state.add_present(tag.tag_id, &prop_def.name, scan);
                }
            }
        }
    }
    let tag_indexes = ctx
        .index_metadata_manager()
        .list_tag_indexes(space_info.space_id)?;
    let ts = ctx.get_write_timestamp()?;
    let (buffer, mark) = match ctx.txn_staging_mark(ts) {
        Ok(mark) => mark,
        Err(error) => {
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
    };
    // Caller-side cross-chunk dedup: storage still validates per-chunk
    // single-batch semantics, but the batch entry rejects a repeated key
    // up front so two chunks never stage the same key under one timestamp.
    let mut seen: HashSet<(LabelId, IdKey)> = HashSet::with_capacity(vertices.len());
    let mut batch_ctx = PrecheckedBatchContext {
        tag_map: &tag_map,
        tag_indexes: &tag_indexes,
        serial_state: &mut serial_state,
        vid_type: &space_info.vid_type,
    };
    let chunk = crate::vertex::MAX_WRITE_SCOPE_KEYS;
    let mut staged_all: Vec<StagedVertexRow> = Vec::with_capacity(vertices.len());
    for window in vertices.chunks(chunk) {
        let mut scope = crate::vertex::WriteScope::new(ts);
        let mut staged: Vec<StagedVertexRow> = Vec::with_capacity(window.len());
        for vertex in window {
            match stage_vertex_row(ctx, space_info.space_id, &mut batch_ctx, vertex, ts) {
                Ok(row) => {
                    let dup_key = (row.label_id, row.key.clone());
                    if !seen.insert(dup_key) {
                        ctx.rollback_staging_to(&buffer, mark);
                        ctx.abort_write_timestamp(ts);
                        return Err(StorageError::vertex_already_exists(format!(
                            "duplicate key in batched write scope: {:?}",
                            row.key
                        )));
                    }
                    staged.push(row);
                }
                Err(e) => {
                    ctx.rollback_staging_to(&buffer, mark);
                    ctx.abort_write_timestamp(ts);
                    return Err(e);
                }
            }
        }
        // Buffer this chunk's rows without touching global state, mirroring
        // the single-batch Phase B grouping.
        let mut label_order: Vec<LabelId> = Vec::new();
        let mut by_label: HashMap<LabelId, Vec<usize>> = HashMap::new();
        for (pos, row) in staged.iter().enumerate() {
            by_label
                .entry(row.label_id)
                .or_insert_with(|| {
                    label_order.push(row.label_id);
                    Vec::new()
                })
                .push(pos);
        }
        let tables: Vec<(LabelId, std::sync::Arc<crate::vertex::ShardedVertexTable>)> =
            match ctx.data_store().with_vertex_tables(|tables| {
                label_order
                    .iter()
                    .map(|label_id| {
                        tables
                            .get(label_id)
                            .cloned()
                            .map(|t| (*label_id, t))
                            .ok_or_else(|| {
                                StorageError::label_not_found(format!("vertex label {}", label_id))
                            })
                    })
                    .collect::<StorageResult<Vec<_>>>()
            }) {
                Ok(tables) => tables,
                Err(e) => {
                    ctx.rollback_staging_to(&buffer, mark);
                    ctx.abort_write_timestamp(ts);
                    return Err(e);
                }
            };
        let mut marks: Vec<Option<StorageResult<()>>> = (0..staged.len()).map(|_| None).collect();
        for (label_id, table) in &tables {
            let positions = &by_label[label_id];
            let mut str_order = Vec::new();
            let mut i64_order = Vec::new();
            for &pos in positions {
                match staged[pos].key {
                    IdKey::Text(_) => str_order.push(pos),
                    IdKey::Int(_) => i64_order.push(pos),
                }
            }
            if !str_order.is_empty() {
                let rows: Vec<(&str, &[(Arc<str>, Value)])> = str_order
                    .iter()
                    .map(|&pos| {
                        let IdKey::Text(ref s) = staged[pos].key else {
                            unreachable!("str_order only holds text keys");
                        };
                        (s.as_str(), staged[pos].props.as_slice())
                    })
                    .collect();
                for (slot, result) in str_order
                    .iter()
                    .zip(table.insert_batch_str_with_scope(&rows, ts, &mut scope))
                {
                    marks[*slot] = Some(result);
                }
            }
            if !i64_order.is_empty() {
                let rows: Vec<(i64, &[(Arc<str>, Value)])> = i64_order
                    .iter()
                    .map(|&pos| {
                        let IdKey::Int(n) = staged[pos].key else {
                            unreachable!("i64_order only holds int keys");
                        };
                        (n, staged[pos].props.as_slice())
                    })
                    .collect();
                for (slot, result) in i64_order
                    .iter()
                    .zip(table.insert_batch_i64_with_scope(&rows, ts, &mut scope))
                {
                    marks[*slot] = Some(result);
                }
            }
        }
        let mut first_error: Option<StorageError> = None;
        for mark_slot in &mut marks {
            if let Some(Err(e)) = mark_slot.take() {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
        if let Some(e) = first_error {
            scope.clear();
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(e);
        }
        if let Err(error) = ctx.absorb_write_scope(&mut scope, ts) {
            ctx.rollback_staging_to(&buffer, mark);
            ctx.abort_write_timestamp(ts);
            return Err(error);
        }
        for row in staged.iter() {
            if let Err(error) = super::index_maintenance::check_vertex_unique_indexes(
                ctx,
                ctx.index_metadata_manager(),
                space_info.space_id,
                &row.vertex_id,
                &row.tag_name,
                &row.props,
            )
            .and_then(|()| record_vertex_insert(ctx, row.vid, Some(row.redo_entry.clone())))
            .and_then(|()| {
                ctx.stage_vertex_index_op(
                    ts,
                    StagedIndexOp::Insert {
                        space_id: space_info.space_id,
                        vid: row.vertex_id.clone(),
                        tag: row.tag_name.clone(),
                        properties: row.props.clone(),
                    },
                )
            }) {
                ctx.rollback_staging_to(&buffer, mark);
                ctx.abort_write_timestamp(ts);
                return Err(error);
            }
        }
        staged_all.extend(staged);
    }
    Ok(staged_all.into_iter().map(|row| row.vid).collect())
}

#[cfg(test)]
mod batch_prefix_tests {
    use super::*;

    #[test]
    fn prefix_error_roundtrips_committed_count() {
        let cause = StorageError::capacity_exceeded();
        let err = batch_prefix_error(4096, 10000, &cause);
        assert_eq!(batch_prefix_committed(&err), Some((4096, 10000)));
        assert!(batch_prefix_committed(&cause).is_none());
    }

    #[test]
    fn single_batch_limit_stays_as_backpressure() {
        // The per-request bound is unchanged; over-limit entries auto-split
        // instead of rejecting, so the constant must stay finite and small.
        assert_eq!(crate::vertex::MAX_WRITE_SCOPE_KEYS, 4096);
    }

    #[test]
    fn over_limit_error_guides_split_batching() {
        let err = over_limit_split_error(crate::vertex::MAX_WRITE_SCOPE_KEYS + 1);
        assert_eq!(
            err.kind(),
            graphdb_core::error::storage::StorageErrorKind::CapacityExceeded
        );
        let message = err.message().to_string();
        assert!(
            message.contains(&crate::vertex::MAX_WRITE_SCOPE_KEYS.to_string()),
            "capacity error must name the limit: {message}"
        );
        assert!(
            message.contains("split"),
            "capacity error must guide split batching: {message}"
        );
    }
}

//! Ordered primary-key existence scan.
//!
//! The planner proves that the cursor key is the destination's unique primary
//! key, that the edge type pins both endpoint labels, and that the index
//! order is global for a pinned generation. This source only walks keys and
//! asks storage for one existence boolean per key; it never materializes an
//! edge or a destination row.

use std::sync::Arc;

use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::wal::EntityRef;
use linkrs_core::DataType;
use linkrs_core::Value;
use parking_lot::RwLock;

use super::super::spec::{BoundIndexPredicate, IndexProjection};
use super::index_scan::build_index_scan_plan;
use super::util::{attach_columnar_stats, reserve_memory, storage_error};
use super::{SourceOperator, SourceOperatorKind};
use crate::executor::streaming::chunk::DataChunk;
use crate::storage::{IndexCursor, IndexRow, IndexScanPlan, QueryStorage};

type Cursor = Box<dyn IndexCursor<Row = IndexRow>>;
type Storage = Arc<RwLock<dyn QueryStorage>>;

/// Keys materialized per cursor open. Batches stay small for early stopping;
/// this bound only keeps repeated opens from re-scanning the index prefix.
const PAGE_ENTRIES: usize = 256;

fn build_plan(
    storage: &Storage,
    space_name: &str,
    index_id: u64,
) -> Result<IndexScanPlan, QueryError> {
    build_index_scan_plan(
        storage,
        space_name,
        index_id,
        &BoundIndexPredicate::Full,
        &IndexProjection::RowIdOnly,
        None,
        0,
        None,
    )
}

fn timing_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("LINKRS_STAGE_TIMING").is_some())
}

fn open_paged(op: &mut SourceOperator, start_after: Option<Vec<u8>>) -> Result<(), QueryError> {
    let build_started = std::time::Instant::now();
    let (storage, plan) = {
        let SourceOperatorKind::OrderedExistenceScan {
            storage,
            space_name,
            index_id,
            ..
        } = &op.kind
        else {
            unreachable!("ordered_existence::open_paged called for a different source");
        };
        let storage = storage.clone().ok_or_else(|| {
            QueryError::execution("OrderedExistenceScan requires storage".to_string())
        })?;
        let plan = build_plan(&storage, space_name, *index_id)?;
        (storage, plan)
    };
    let build_us = build_started.elapsed().as_micros() as u64;
    let opened = std::time::Instant::now();
    let guard = storage.read();
    let lock_us = opened.elapsed().as_micros() as u64;
    let cursor_started = std::time::Instant::now();
    let cursor = guard
        .open_ordered_index_cursor(&plan, start_after, PAGE_ENTRIES)
        .map_err(|error| {
            storage_error(
                "OrderedExistenceScan",
                "open paged cursor",
                &plan.space,
                error,
            )
        })?;
    drop(guard);
    let cursor_us = cursor_started.elapsed().as_micros() as u64;
    if timing_enabled() {
        eprintln!(
            "[stage] ordered_open build_us={build_us} lock_us={lock_us} cursor_us={cursor_us}"
        );
    }
    if let Some(stats) = op.runtime.as_ref().map(|rt| rt.columnar_stats()) {
        let ordering = std::sync::atomic::Ordering::Relaxed;
        stats
            .ordered_existence_open_us
            .fetch_add(opened.elapsed().as_micros() as u64, ordering);
        stats.ordered_existence_opens.fetch_add(1, ordering);
    }
    if let SourceOperatorKind::OrderedExistenceScan { cursor: slot, .. } = &mut op.kind {
        *slot = Some(cursor);
    }
    Ok(())
}

pub(crate) fn open(op: &mut SourceOperator) -> Result<(), QueryError> {
    let (storage, plan, dst_tag, key_property, generation) = {
        let SourceOperatorKind::OrderedExistenceScan {
            storage,
            space_name,
            index_id,
            dst_tag,
            key_property,
            generation,
            ..
        } = &op.kind
        else {
            unreachable!("ordered_existence::open called for a different source");
        };
        let storage = storage.clone().ok_or_else(|| {
            QueryError::execution("OrderedExistenceScan requires storage".to_string())
        })?;
        let plan = build_plan(&storage, space_name, *index_id)?;
        (
            storage,
            plan,
            dst_tag.clone(),
            key_property.clone(),
            *generation,
        )
    };
    open_paged(op, None)?;

    let guard = storage.read();
    let snapshot = guard.snapshot_handle();
    let generation_stable = guard
        .ordered_index_proof(&plan.space, &dst_tag, &key_property)
        .ok()
        .flatten()
        .is_some_and(|proof| proof.generation == generation);
    drop(guard);

    if let SourceOperatorKind::OrderedExistenceScan {
        emitted,
        exhausted,
        early_stop,
        snapshot: stored_snapshot,
        resume_key,
        ..
    } = &mut op.kind
    {
        *emitted = 0;
        *exhausted = false;
        *early_stop = snapshot.is_some() && generation_stable;
        *stored_snapshot = snapshot;
        *resume_key = None;
    }
    if op_early_stop(op) {
        return Ok(());
    }
    record_drift(op);
    Ok(())
}

pub(crate) fn next(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    let stats = op.runtime.as_ref().map(|rt| rt.columnar_stats());
    let (storage, space_name, key_type, limit, batch_size, src_tag, dst_tag, edge_type, snapshot) = {
        let SourceOperatorKind::OrderedExistenceScan {
            storage,
            space_name,
            key_type,
            limit,
            batch_size,
            src_tag,
            dst_tag,
            edge_type,
            snapshot,
            early_stop,
            emitted,
            exhausted,
            ..
        } = &op.kind
        else {
            unreachable!("ordered_existence::next called for a different source");
        };
        let storage = storage.clone().ok_or_else(|| {
            QueryError::execution("OrderedExistenceScan requires storage".to_string())
        })?;
        if *exhausted || (*early_stop && *emitted >= *limit) {
            return Ok(None);
        }
        (
            storage,
            space_name.clone(),
            key_type.clone(),
            *limit,
            *batch_size,
            src_tag.clone(),
            dst_tag.clone(),
            edge_type.clone(),
            *snapshot,
        )
    };
    let current_snapshot = storage.read().snapshot_handle();
    if current_snapshot != snapshot {
        disable_early_stop(op, stats.as_deref());
    }
    if let Some(resume) = take_resume_key(op) {
        open_paged(op, Some(resume))?;
    }

    loop {
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if let Some(resume) = take_resume_key(op) {
            open_paged(op, Some(resume))?;
        }
        let mut cursor = take_cursor(op)?;
        let cursor_started = std::time::Instant::now();
        let rows = cursor.next_batch(batch_size).map_err(|error| {
            storage_error("OrderedExistenceScan", "read cursor", &space_name, error)
        })?;
        let exhausted = cursor.is_exhausted();
        let last_key = cursor.last_key();
        if let Some(stats) = &stats {
            stats.ordered_existence_cursor_us.fetch_add(
                cursor_started.elapsed().as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        restore_cursor(op, cursor);
        if let Some(key) = last_key {
            set_resume_key(op, key);
        }

        let mut vids = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(vid) = vertex_id(row, &space_name)? {
                vids.push(vid);
            }
        }
        if let Some(stats) = &stats {
            stats
                .ordered_existence_candidates
                .fetch_add(vids.len() as u64, std::sync::atomic::Ordering::Relaxed);
        }
        let gate_started = std::time::Instant::now();
        let exists = storage.read().incoming_edge_exists_batch(
            &space_name,
            &vids,
            &src_tag,
            &dst_tag,
            &edge_type,
        )?;
        if let Some(stats) = &stats {
            stats.ordered_existence_gate_us.fetch_add(
                gate_started.elapsed().as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        let (output_rows, matched, emitted_now) = select_emitted(
            vids,
            exists,
            emitted(op),
            limit,
            op_early_stop(op),
            &key_type,
        );
        if let Some(stats) = &stats {
            let ordering = std::sync::atomic::Ordering::Relaxed;
            stats.ordered_existence_matches.fetch_add(matched, ordering);
            stats
                .ordered_existence_output_rows
                .fetch_add(emitted_now as u64, ordering);
        }
        add_emitted(op, emitted_now);
        if output_rows.is_empty() {
            if exhausted && !has_resume_key(op) {
                set_exhausted(op);
                return Ok(None);
            }
            continue;
        }
        let reservation = reserve_memory(&op.runtime, &output_rows)?;
        let chunk = DataChunk::new_with_layout(output_rows, op.output_layout.clone());
        let chunk = attach_columnar_stats(&op.runtime, chunk);
        let chunk = if let Some(reservation) = reservation {
            chunk.with_memory_reservation(reservation)
        } else {
            chunk
        };
        return Ok(Some(chunk));
    }
}

fn select_emitted(
    vids: Vec<VertexId>,
    exists: Vec<bool>,
    emitted_before: usize,
    limit: usize,
    early_stop: bool,
    key_type: &DataType,
) -> (Vec<Vec<Value>>, u64, usize) {
    let mut output_rows = Vec::new();
    let mut matched = 0u64;
    let mut emitted_now = 0usize;
    for (vid, found) in vids.into_iter().zip(exists) {
        if !found {
            continue;
        }
        matched += 1;
        let Some(key) = identity_key_value(vid, key_type) else {
            continue;
        };
        output_rows.push(vec![key]);
        emitted_now += 1;
        if early_stop && emitted_before + emitted_now >= limit {
            break;
        }
    }
    (output_rows, matched, emitted_now)
}

fn identity_key_value(vid: VertexId, data_type: &DataType) -> Option<Value> {
    let id = vid.as_int64()?;
    match data_type {
        DataType::BigInt => Some(Value::BigInt(id)),
        DataType::Int => i32::try_from(id).ok().map(Value::Int),
        DataType::SmallInt => i16::try_from(id).ok().map(Value::SmallInt),
        _ => None,
    }
}

fn vertex_id(row: IndexRow, space_name: &str) -> Result<Option<VertexId>, QueryError> {
    let entity_ref = match row {
        IndexRow::RowId(entity_ref) => entity_ref,
        IndexRow::Covering { entity_ref, .. } => entity_ref,
    };
    match entity_ref {
        EntityRef::Vertex(vid) => Ok(Some(vid)),
        EntityRef::Edge { .. } => Err(QueryError::execution(format!(
            "OrderedExistenceScan received an edge index row in space {space_name}"
        ))),
    }
}

fn take_cursor(op: &mut SourceOperator) -> Result<Cursor, QueryError> {
    if let SourceOperatorKind::OrderedExistenceScan { cursor, .. } = &mut op.kind {
        cursor.take().ok_or_else(|| {
            QueryError::execution("OrderedExistenceScan cursor is missing".to_string())
        })
    } else {
        unreachable!("ordered existence helper called for a different source")
    }
}

fn restore_cursor(op: &mut SourceOperator, cursor: Cursor) {
    if let SourceOperatorKind::OrderedExistenceScan { cursor: slot, .. } = &mut op.kind {
        *slot = Some(cursor);
    }
}

fn op_early_stop(op: &SourceOperator) -> bool {
    match &op.kind {
        SourceOperatorKind::OrderedExistenceScan { early_stop, .. } => *early_stop,
        _ => false,
    }
}

fn emitted(op: &SourceOperator) -> usize {
    match &op.kind {
        SourceOperatorKind::OrderedExistenceScan { emitted, .. } => *emitted,
        _ => 0,
    }
}

fn add_emitted(op: &mut SourceOperator, count: usize) {
    if let SourceOperatorKind::OrderedExistenceScan { emitted, .. } = &mut op.kind {
        *emitted += count;
    }
}

fn has_resume_key(op: &SourceOperator) -> bool {
    matches!(
        &op.kind,
        SourceOperatorKind::OrderedExistenceScan {
            resume_key: Some(_),
            ..
        }
    )
}

fn take_resume_key(op: &mut SourceOperator) -> Option<Vec<u8>> {
    match &mut op.kind {
        SourceOperatorKind::OrderedExistenceScan { resume_key, .. } => resume_key.take(),
        _ => None,
    }
}

fn set_resume_key(op: &mut SourceOperator, key: Vec<u8>) {
    if let SourceOperatorKind::OrderedExistenceScan { resume_key, .. } = &mut op.kind {
        *resume_key = Some(key);
    }
}

fn set_exhausted(op: &mut SourceOperator) {
    if let SourceOperatorKind::OrderedExistenceScan { exhausted, .. } = &mut op.kind {
        *exhausted = true;
    }
}

fn disable_early_stop(
    op: &mut SourceOperator,
    stats: Option<&crate::executor::streaming::runtime::ColumnarStats>,
) {
    if matches!(
        &op.kind,
        SourceOperatorKind::OrderedExistenceScan {
            early_stop: false,
            ..
        }
    ) {
        return;
    }
    if let SourceOperatorKind::OrderedExistenceScan { early_stop, .. } = &mut op.kind {
        *early_stop = false;
    }
    record_drift_with(op, stats);
}

fn record_drift(op: &mut SourceOperator) {
    record_drift_with(
        op,
        op.runtime.as_ref().map(|rt| rt.columnar_stats()).as_deref(),
    );
}

fn record_drift_with(
    op: &SourceOperator,
    stats: Option<&crate::executor::streaming::runtime::ColumnarStats>,
) {
    if let Some(stats) = stats {
        stats
            .ordered_existence_drift
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let _ = op;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_of(vid: VertexId) -> i64 {
        vid.as_int64().expect("test vertex id")
    }

    fn walk_keys(limit: usize, present: impl Fn(i64) -> bool, early_stop: bool) -> Vec<i64> {
        let batch_size = limit.clamp(1, 16);
        let mut emitted = 0usize;
        let mut output = Vec::new();
        for batch in (1..=100i64).collect::<Vec<_>>().chunks(batch_size) {
            let vids = batch
                .iter()
                .map(|id| VertexId::try_from_int64(*id).expect("test vertex id"))
                .collect::<Vec<_>>();
            let exists = batch.iter().map(|id| present(*id)).collect::<Vec<_>>();
            let (rows, _, emitted_now) =
                select_emitted(vids, exists, emitted, limit, early_stop, &DataType::BigInt);
            emitted += emitted_now;
            output.extend(rows.into_iter().map(|row| match &row[0] {
                Value::BigInt(value) => *value,
                other => panic!("unexpected key {other:?}"),
            }));
            if early_stop && emitted >= limit {
                break;
            }
        }
        output
    }

    #[test]
    fn ordered_existence_emits_sorted_prefix_for_each_batch_limit() {
        for limit in [1, 10, 17, 100] {
            let dense = walk_keys(limit, |_| true, true);
            assert_eq!(dense, (1..=limit.min(100) as i64).collect::<Vec<_>>());

            let sparse = walk_keys(limit, |id| id % 3 == 0, true);
            let expected = (1..=100)
                .filter(|id| id % 3 == 0)
                .take(limit)
                .collect::<Vec<_>>();
            assert_eq!(sparse, expected);

            assert!(walk_keys(limit, |_| false, true).is_empty());
            for key in &sparse {
                assert_eq!(key % 3, 0);
            }
            assert!(sparse.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn ordered_existence_without_early_stop_keeps_every_existing_key() {
        for limit in [1, 10, 17, 100] {
            let rows = walk_keys(limit, |id| id % 4 == 1, false);
            let expected = (1..=100).filter(|id| id % 4 == 1).collect::<Vec<_>>();
            assert_eq!(rows, expected);
            assert!(rows
                .iter()
                .all(|id| key_of(VertexId::try_from_int64(*id).expect("test vertex id")) == *id));
        }
    }
}

use std::sync::Arc;

use crate::core::{error::QueryError, Value};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::state::GlobalState;
use crate::storage::open_edge_scan;
use crate::storage::open_vertex_scan;
use crate::storage::{RequiredProperty, ScanOptions, StorageError};

use super::SourceOperator;
use super::util::{
    attach_columnar_stats, make_flat_edge_row, make_flat_vertex_record_row, make_flat_vertex_row,
    reserve_memory_with_extra, storage_error,
};

/// Open the storage-backed scan source variants, creating the cursor that
/// streams batches from storage.
pub(crate) fn open(op: &mut SourceOperator, base: &mut OperatorBase) -> Result<(), QueryError> {
    match op {
        SourceOperator::StorageScanVertices {
            storage,
            space_name,
            limit,
            partition_range,
            col_names,
            projected_properties,
            predicate,
            cursor,
        } => {
            let storage_ref = storage.as_ref().ok_or_else(|| {
                QueryError::execution("StorageScanVertices requires storage".to_string())
            })?;
            *cursor = Some(
                open_vertex_scan(
                    storage_ref,
                    space_name,
                    &ScanOptions {
                        limit: *limit,
                        vertex_id_range: partition_range.clone(),
                        projection: (!projected_properties.is_empty()).then(|| {
                            projected_properties
                                .iter()
                                .map(|n| RequiredProperty::new(n.clone()))
                                .collect()
                        }),
                        predicate: (!predicate.is_empty()).then(|| predicate.clone()),
                        ..ScanOptions::default()
                    },
                )
                .map_err(|error| {
                    storage_error("StorageScanVertices", "open cursor", space_name, error)
                })?,
            );
            base.insert_state(GlobalState::Source(SourceState::StorageScanVertices {
                partition_id: base.partition_id.unwrap_or(0),
                partition_range: partition_range.clone(),
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            }));
        }
        SourceOperator::StorageScanEdges {
            storage,
            space_name,
            limit,
            edge_type,
            partition_range,
            col_names,
            projected_properties,
            cursor,
        } => {
            let storage_ref = storage.as_ref().ok_or_else(|| {
                QueryError::execution("StorageScanEdges requires storage".to_string())
            })?;
            *cursor = Some(
                open_edge_scan(
                    storage_ref,
                    space_name,
                    &ScanOptions {
                        limit: *limit,
                        edge_type: edge_type.clone(),
                        edge_src_id_range: partition_range.clone(),
                        projection: (!projected_properties.is_empty()).then(|| {
                            projected_properties
                                .iter()
                                .map(|n| RequiredProperty::new(n.clone()))
                                .collect()
                        }),
                        ..ScanOptions::default()
                    },
                )
                .map_err(|error| {
                    storage_error("StorageScanEdges", "open cursor", space_name, error)
                })?,
            );
            base.insert_state(GlobalState::Source(SourceState::StorageScanEdges {
                partition_id: base.partition_id.unwrap_or(0),
                partition_range: partition_range.clone(),
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            }));
        }
        _ => unreachable!("storage_scan::open called for a non-scan source"),
    }
    Ok(())
}

/// Emit the next chunk from the storage cursor, translating rows into the
/// single-entity column layout.
pub(crate) fn next(
    op: &mut SourceOperator,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    match op {
        SourceOperator::StorageScanVertices {
            space_name,
            cursor,
            projected_properties,
            ..
        } => {
            let flatten = projected_properties.clone();
            if flatten.is_empty() {
                next_cursor_chunk(
                    cursor,
                    space_name,
                    "StorageScanVertices",
                    base,
                    move |vertex| make_flat_vertex_row(vertex, &flatten),
                    |cur, batch_size| cur.next_batch(batch_size),
                )
            } else {
                // Flat path: pull records directly from storage (skipping
                // per-row Vertex/HashMap boxing) and widen them into the flat
                // property layout.
                next_cursor_chunk(
                    cursor,
                    space_name,
                    "StorageScanVertices",
                    base,
                    move |record| make_flat_vertex_record_row(record, &flatten),
                    |cur, batch_size| cur.next_flat_batch(batch_size),
                )
            }
        }
        SourceOperator::StorageScanEdges {
            space_name,
            cursor,
            projected_properties,
            ..
        } => {
            let flatten = projected_properties.clone();
            next_cursor_chunk(
                cursor,
                space_name,
                "StorageScanEdges",
                base,
                move |edge| make_flat_edge_row(edge, &flatten),
                |cur, batch_size| cur.next_batch(batch_size),
            )
        }
        _ => unreachable!("storage_scan::next called for a non-scan source"),
    }
}

/// Shared pull loop over a storage cursor: read a batch, translate each row
/// into the entity layout, then emit a chunk with a memory reservation.
///
/// The produced chunk eagerly builds its typed column layout from the batch
/// rows (fixed-size scalar columns only; NULL/mixed/string columns fall
/// back), and the extra typed allocation is accounted in the chunk's memory
/// reservation.
fn next_cursor_chunk<C, R, FRow, FBatch>(
    cursor: &mut Option<C>,
    space_name: &str,
    source: &str,
    base: &mut OperatorBase,
    mut map_row: FRow,
    mut pull_batch: FBatch,
) -> Result<Option<DataChunk>, QueryError>
where
    FRow: FnMut(R) -> Vec<Value>,
    FBatch: FnMut(&mut C, usize) -> Result<Vec<R>, StorageError>,
{
    loop {
        base.ensure_not_cancelled()?;
        let mut cur = match cursor.take() {
            Some(c) => c,
            None => return Ok(None),
        };
        let batch = pull_batch(&mut cur, base.chunk_size)
            .map_err(|error| storage_error(source, "read cursor", space_name, error))?;
        if batch.is_empty() {
            return Ok(None);
        }
        let rows = batch.into_iter().map(&mut map_row).collect::<Vec<_>>();
        if !rows.is_empty() {
            // No proactive Value columnar materialisation — the `columns`
            // cache is lazily built by the first `get_column` consumer. The
            // typed layout IS built eagerly here so the typed batch evaluator
            // and index-based access stay available across selection
            // boundaries.
            let mut chunk = DataChunk::new_with_layout(rows, Arc::clone(&base.output_layout));
            let typed_bytes = chunk.build_typed_columns();
            let reservation = reserve_memory_with_extra(base, &chunk.rows, typed_bytes)?;
            let chunk = attach_columnar_stats(base, chunk);
            let chunk = if let Some(r) = reservation {
                chunk.with_memory_reservation(r)
            } else {
                chunk
            };
            *cursor = Some(cur);
            return Ok(Some(chunk));
        }
        *cursor = Some(cur);
    }
}
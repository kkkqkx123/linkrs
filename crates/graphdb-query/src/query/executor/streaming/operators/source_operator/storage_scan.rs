use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::streaming::chunk::{DataChunk, TypedColumn};
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::state::GlobalState;
use crate::storage::open_edge_scan;
use crate::storage::open_vertex_scan;
use crate::storage::{
    EdgeColumnBatch, RequiredProperty, ScanOptions, StorageError, VertexColumnBatch,
};

use super::util::{
    attach_columnar_stats, make_flat_edge_row, make_flat_vertex_record_row, make_flat_vertex_row,
    reserve_memory_with_extra, storage_error,
};
use super::SourceOperator;
use super::SourceOperatorKind;

/// Runtime switch: storage column-block scan mode (A1).
///
/// Rollback knob — set to `false` to keep the row-based scan path exactly as
/// before the column-block path existed. Default off.
static COLUMN_BLOCK_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable or disable the storage column-block scan mode (A1).
pub fn set_column_block_enabled(enabled: bool) {
    COLUMN_BLOCK_ENABLED.store(enabled, AtomicOrdering::Relaxed);
}

/// Whether the storage column-block scan mode is currently enabled.
pub fn column_block_enabled() -> bool {
    COLUMN_BLOCK_ENABLED.load(AtomicOrdering::Relaxed)
}

/// Open the storage-backed scan source variants, creating the cursor that
/// streams batches from storage.
pub(crate) fn open(op: &mut SourceOperator) -> Result<(), QueryError> {
    let state = match &mut op.kind {
        SourceOperatorKind::StorageScanVertices {
            storage,
            space_name,
            limit,
            partition_range,
            col_names,
            projected_properties,
            predicate,
            tag,
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
                        tag: tag.clone(),
                        column_block_mode: column_block_enabled(),
                        ..ScanOptions::default()
                    },
                )
                .map_err(|error| {
                    storage_error("StorageScanVertices", "open cursor", space_name, error)
                })?,
            );
            GlobalState::Source(SourceState::StorageScanVertices {
                partition_id: op.config.partition_id.unwrap_or(0),
                partition_range: partition_range.clone(),
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            })
        }
        SourceOperatorKind::StorageScanEdges {
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
            GlobalState::Source(SourceState::StorageScanEdges {
                partition_id: op.config.partition_id.unwrap_or(0),
                partition_range: partition_range.clone(),
                cursor: None,
                buffer: Vec::new(),
                current_index: 0,
                col_names: col_names.clone(),
            })
        }
        _ => unreachable!("storage_scan::open called for a non-scan source"),
    };
    op.insert_state(state);
    Ok(())
}

/// Emit the next chunk from the storage cursor, translating rows into the
/// single-entity column layout.
pub(crate) fn next(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
    let is_vertex_scan = matches!(&op.kind, SourceOperatorKind::StorageScanVertices { .. });
    let flatten = match &op.kind {
        SourceOperatorKind::StorageScanVertices {
            projected_properties,
            ..
        }
        | SourceOperatorKind::StorageScanEdges {
            projected_properties,
            ..
        } => projected_properties.clone(),
        _ => unreachable!("storage_scan::next called for a non-scan source"),
    };
    if column_block_enabled() {
        if is_vertex_scan {
            return next_column_chunk(op, "StorageScanVertices", &flatten);
        }
        return next_edge_column_chunk(op, "StorageScanEdges", &flatten);
    }
    if is_vertex_scan {
        let (cursor, space_name) = match &mut op.kind {
            SourceOperatorKind::StorageScanVertices {
                space_name, cursor, ..
            } => (cursor, &*space_name),
            _ => unreachable!("storage_scan::next called for a non-vertex scan"),
        };
        if flatten.is_empty() {
            next_cursor_chunk_inner(
                cursor,
                space_name,
                "StorageScanVertices",
                &op.runtime,
                op.config.chunk_size,
                &op.output_layout,
                move |vertex| make_flat_vertex_row(vertex, &flatten),
                |cur: &mut Box<dyn crate::storage::VertexCursor>, batch_size| {
                    cur.next_batch(batch_size)
                },
            )
        } else {
            // Flat path: pull records directly from storage (skipping
            // per-row Vertex/HashMap boxing) and widen them into the flat
            // property layout.
            next_cursor_chunk_inner(
                cursor,
                space_name,
                "StorageScanVertices",
                &op.runtime,
                op.config.chunk_size,
                &op.output_layout,
                move |record| make_flat_vertex_record_row(record, &flatten),
                |cur: &mut Box<dyn crate::storage::VertexCursor>, batch_size| {
                    cur.next_flat_batch(batch_size)
                },
            )
        }
    } else {
        let (cursor, space_name) = match &mut op.kind {
            SourceOperatorKind::StorageScanEdges {
                space_name, cursor, ..
            } => (cursor, &*space_name),
            _ => unreachable!("storage_scan::next called for a non-edge scan"),
        };
        next_cursor_chunk_inner(
            cursor,
            space_name,
            "StorageScanEdges",
            &op.runtime,
            op.config.chunk_size,
            &op.output_layout,
            move |edge| make_flat_edge_row(edge, &flatten),
            |cur: &mut Box<dyn crate::storage::EdgeCursor>, batch_size| cur.next_batch(batch_size),
        )
    }
}

/// Shared pull loop over a storage cursor: read a batch, translate each row
/// into the entity layout, then emit a chunk with a memory reservation.
///
/// The produced chunk eagerly builds its typed column layout from the batch
/// rows (fixed-size scalar columns only; NULL/mixed/string columns fall
/// back), and the extra typed allocation is accounted in the chunk's memory
/// reservation.
fn next_cursor_chunk_inner<C, R, FRow, FBatch>(
    cursor: &mut Option<C>,
    space_name: &str,
    source: &str,
    runtime: &Option<Arc<ExecutionRuntime>>,
    chunk_size: usize,
    output_layout: &Arc<SlotLayout>,
    mut map_row: FRow,
    mut pull_batch: FBatch,
) -> Result<Option<DataChunk>, QueryError>
where
    FRow: FnMut(R) -> Vec<Value>,
    FBatch: FnMut(&mut C, usize) -> Result<Vec<R>, StorageError>,
{
    loop {
        if let Some(rt) = runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        let mut cur = match cursor.take() {
            Some(c) => c,
            None => return Ok(None),
        };
        let batch = pull_batch(&mut cur, chunk_size)
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
            let mut chunk = DataChunk::new_with_layout(rows, Arc::clone(output_layout));
            let typed_bytes = chunk.build_typed_columns(use_columnar_path(runtime));
            let reservation = reserve_memory_with_extra(runtime, &chunk.rows, typed_bytes)?;
            let chunk = attach_columnar_stats(runtime, chunk);
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

/// Whether the typed columnar layout should be used for this operator's
/// chunks.
///
/// The global runtime switch remains a forced override; the shared
/// [`ColumnarPolicy`] provides the adaptive decision.  The policy is only
/// mutated between queries (stats merge at query completion), so the
/// decision is stable for the whole query even though it is read per chunk.
fn use_columnar_path(runtime: &Option<Arc<ExecutionRuntime>>) -> bool {
    if !crate::query::executor::streaming::chunk::typed_columns_enabled() {
        return false;
    }
    runtime
        .as_ref()
        .and_then(|runtime| runtime.columnar_policy())
        .is_none_or(|policy| policy.should_use_columnar())
}

/// Column-block pull loop over a storage cursor (A1).
///
/// Pulls a [`VertexColumnBatch`], builds the chunk's typed column layout
/// directly from the batch columns (skipping the intermediate per-row
/// `Vec<Value>` materialization that `build_typed_columns` would re-read),
/// and accounts the typed allocation in the chunk's memory reservation.
fn next_column_chunk(
    op: &mut SourceOperator,
    source: &str,
    projected_properties: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    let (cursor, space_name) = match &mut op.kind {
        SourceOperatorKind::StorageScanVertices {
            space_name, cursor, ..
        } => (cursor, &*space_name),
        _ => unreachable!("next_column_chunk called for a non-vertex scan"),
    };
    if let Some(rt) = op.runtime.as_ref() {
        rt.ensure_not_cancelled()?;
    }
    let mut cur = match cursor.take() {
        Some(c) => c,
        None => return Ok(None),
    };
    let batch = cur
        .next_column_batch(projected_properties, op.config.chunk_size)
        .map_err(|error| storage_error(source, "read column batch", space_name, error))?;
    if batch.is_empty() {
        return Ok(None);
    }
    let chunk = build_column_chunk(&op.runtime, &op.output_layout, batch, projected_properties)?;
    *cursor = Some(cur);
    Ok(Some(chunk))
}

/// Assemble a [`DataChunk`] from a [`VertexColumnBatch`], building the typed
/// column layout straight from the batch columns.
fn build_column_chunk(
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
    batch: VertexColumnBatch,
    flatten: &[String],
) -> Result<DataChunk, QueryError> {
    let layout = Arc::clone(output_layout);
    let row_count = batch.len();

    // Pre-compute per-column `Value` vectors once (used for both the rows and
    // the fallback typed columns).
    let mut prop_values: Vec<Vec<Value>> = Vec::with_capacity(batch.columns.len());
    for column in &batch.columns {
        prop_values.push(
            (0..row_count)
                .map(|row| {
                    column
                        .values
                        .value_at(row)
                        .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
                })
                .collect(),
        );
    }

    let mut rows = Vec::with_capacity(row_count);
    for (row, tag_name) in batch.tag_names.iter().enumerate() {
        let mut properties = std::collections::HashMap::with_capacity(batch.columns.len());
        for (index, column) in batch.columns.iter().enumerate() {
            let value = &prop_values[index][row];
            if !matches!(value, Value::Null(_)) {
                properties.insert(column.name.clone(), value.clone());
            }
        }
        let tags = if tag_name.is_empty() {
            Vec::new()
        } else {
            vec![crate::core::Tag::new(tag_name.clone(), properties.clone())]
        };
        let vertex = crate::core::Vertex {
            vid: batch.vids[row],
            id: batch.internal_ids[row],
            tags,
            properties,
        };
        let mut row_vec = Vec::with_capacity(flatten.len() + 1);
        // Replicates Vertex::property_value semantics: vertex property first,
        // then a tag whose name equals the property yields the tag's property
        // map, otherwise null.
        let flat_values: Vec<Value> = flatten
            .iter()
            .map(|prop| {
                vertex
                    .property_value(prop)
                    .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
            })
            .collect();
        row_vec.push(Value::Vertex(Box::new(vertex)));
        row_vec.extend(flat_values);
        rows.push(row_vec);
    }

    let mut chunk = DataChunk::new_with_layout(rows, layout);
    if use_columnar_path(runtime) {
        let mut typed: Vec<TypedColumn> = Vec::with_capacity(output_layout.len());
        typed.push(TypedColumn::Fallback(
            chunk.rows.iter().map(|r| r[0].clone()).collect(),
        ));
        for prop in flatten {
            match batch.columns.iter().position(|c| c.name == *prop) {
                Some(index) => typed.push(typed_from_column(
                    &batch.columns[index].values,
                    &prop_values[index],
                )),
                None => typed.push(TypedColumn::Fallback(
                    (0..row_count)
                        .map(|_| Value::Null(crate::core::value::NullType::Null))
                        .collect(),
                )),
            }
        }
        chunk.typed_columns = Some(typed);
    }

    if let Some(runtime) = runtime.as_ref() {
        runtime.columnar_stats().record_column_block_hit();
    }

    let typed_bytes = chunk
        .typed_columns
        .as_ref()
        .map(|cols| cols.iter().map(TypedColumn::estimated_size).sum())
        .unwrap_or(0);
    let reservation = reserve_memory_with_extra(runtime, &chunk.rows, typed_bytes)?;
    let chunk = attach_columnar_stats(runtime, chunk);
    Ok(if let Some(r) = reservation {
        chunk.with_memory_reservation(r)
    } else {
        chunk
    })
}

/// Convert a storage [`ColumnValues`] into the chunk's [`TypedColumn`].
fn typed_from_column(values: &crate::storage::ColumnValues, fallback: &[Value]) -> TypedColumn {
    match values {
        crate::storage::ColumnValues::I64 { values: v, .. } if values.all_valid() => {
            TypedColumn::I64(v.clone())
        }
        crate::storage::ColumnValues::F64 { values: v, .. } if values.all_valid() => {
            TypedColumn::F64(v.clone())
        }
        crate::storage::ColumnValues::I32 { values: v, .. } if values.all_valid() => {
            TypedColumn::I32(v.clone())
        }
        // General columns that happen to be uniform Date/String values are
        // promoted to the typed layout so filtering stays vectorized.
        crate::storage::ColumnValues::General { .. } => {
            let mut dates = Vec::with_capacity(fallback.len());
            let mut strings = Vec::with_capacity(fallback.len());
            let mut is_date = true;
            let mut is_string = true;
            for v in fallback {
                match v {
                    Value::Date(d) => dates.push(d.to_days()),
                    _ => is_date = false,
                }
                match v {
                    Value::String(s) => strings.push(Arc::from(s.as_str())),
                    _ => is_string = false,
                }
            }
            if is_date {
                TypedColumn::Date(dates)
            } else if is_string {
                TypedColumn::Utf8(strings)
            } else {
                TypedColumn::Fallback(fallback.to_vec())
            }
        }
        _ => TypedColumn::Fallback(fallback.to_vec()),
    }
}

/// Column-block pull loop for edges (A1).
///
/// Mirrors [`next_column_chunk`]: pulls an [`EdgeColumnBatch`] from the
/// cursor and assembles the chunk directly from the batch columns.
fn next_edge_column_chunk(
    op: &mut SourceOperator,
    source: &str,
    projected_properties: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    let (cursor, space_name) = match &mut op.kind {
        SourceOperatorKind::StorageScanEdges {
            space_name, cursor, ..
        } => (cursor, &*space_name),
        _ => unreachable!("next_edge_column_chunk called for a non-edge scan"),
    };
    if let Some(rt) = op.runtime.as_ref() {
        rt.ensure_not_cancelled()?;
    }
    let mut cur = match cursor.take() {
        Some(c) => c,
        None => return Ok(None),
    };
    let batch = cur
        .next_column_batch(projected_properties, op.config.chunk_size)
        .map_err(|error| storage_error(source, "read edge column batch", space_name, error))?;
    if batch.is_empty() {
        return Ok(None);
    }
    let chunk =
        build_edge_column_chunk(&op.runtime, &op.output_layout, batch, projected_properties)?;
    *cursor = Some(cur);
    Ok(Some(chunk))
}

/// Assemble a [`DataChunk`] from an [`EdgeColumnBatch`], building the typed
/// column layout straight from the batch columns.
fn build_edge_column_chunk(
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
    batch: EdgeColumnBatch,
    flatten: &[String],
) -> Result<DataChunk, QueryError> {
    let layout = Arc::clone(output_layout);
    let row_count = batch.len();

    // Pre-compute per-column `Value` vectors once (used for both the rows and
    // the fallback typed columns).
    let mut prop_values: Vec<Vec<Value>> = Vec::with_capacity(batch.columns.len());
    for column in &batch.columns {
        prop_values.push(
            (0..row_count)
                .map(|row| {
                    column
                        .values
                        .value_at(row)
                        .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
                })
                .collect(),
        );
    }

    let mut rows = Vec::with_capacity(row_count);
    for (row, (src, dst, edge_type, ranking)) in batch
        .srcs
        .iter()
        .zip(&batch.dsts)
        .zip(&batch.edge_types)
        .zip(&batch.rankings)
        .map(|(((src, dst), edge_type), ranking)| (src, dst, edge_type, ranking))
        .enumerate()
    {
        let mut properties = std::collections::HashMap::with_capacity(batch.columns.len());
        for (index, column) in batch.columns.iter().enumerate() {
            let value = &prop_values[index][row];
            if !matches!(value, Value::Null(_)) {
                properties.insert(column.name.clone(), value.clone());
            }
        }
        let edge = crate::core::Edge {
            src: *src,
            dst: *dst,
            edge_type: edge_type.clone(),
            ranking: *ranking,
            props: properties,
        };
        let flat_values: Vec<Value> = flatten
            .iter()
            .map(|prop| {
                edge.get_property(prop)
                    .cloned()
                    .unwrap_or_else(|| Value::Null(crate::core::value::NullType::Null))
            })
            .collect();
        let mut row_vec = Vec::with_capacity(flatten.len() + 1);
        row_vec.push(Value::Edge(Box::new(edge)));
        row_vec.extend(flat_values);
        rows.push(row_vec);
    }

    let mut chunk = DataChunk::new_with_layout(rows, layout);
    if use_columnar_path(runtime) {
        let mut typed: Vec<TypedColumn> = Vec::with_capacity(output_layout.len());
        typed.push(TypedColumn::Fallback(
            chunk.rows.iter().map(|r| r[0].clone()).collect(),
        ));
        for prop in flatten {
            match batch.columns.iter().position(|c| c.name == *prop) {
                Some(index) => typed.push(typed_from_column(
                    &batch.columns[index].values,
                    &prop_values[index],
                )),
                None => typed.push(TypedColumn::Fallback(
                    (0..row_count)
                        .map(|_| Value::Null(crate::core::value::NullType::Null))
                        .collect(),
                )),
            }
        }
        chunk.typed_columns = Some(typed);
    }

    if let Some(runtime) = runtime.as_ref() {
        runtime.columnar_stats().record_column_block_hit();
    }

    let typed_bytes = chunk
        .typed_columns
        .as_ref()
        .map(|cols| cols.iter().map(TypedColumn::estimated_size).sum())
        .unwrap_or(0);
    let reservation = reserve_memory_with_extra(runtime, &chunk.rows, typed_bytes)?;
    let chunk = attach_columnar_stats(runtime, chunk);
    Ok(if let Some(r) = reservation {
        chunk.with_memory_reservation(r)
    } else {
        chunk
    })
}

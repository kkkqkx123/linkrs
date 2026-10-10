use std::sync::Arc;

use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{gather_typed_column, DataChunk, TypedColumn};
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::Value;

use super::expand_buffer::visible_rows;

/// Read the seed id array when the seed slot carries a vertex identity
/// column, so seed parsing skips cloning and parsing the row value.
/// Typed columns stay in lockstep with rows across selection and gather
/// transforms, so the absolute row index addresses the id array directly.
pub(super) fn identity_seed_ids(chunk: &DataChunk, seed_slot: usize) -> Option<&[VertexId]> {
    match chunk.typed_column(seed_slot) {
        Some(TypedColumn::VertexIdentity(ids)) => Some(ids),
        _ => None,
    }
}

/// Resolve the seed id for one visible row: identity scan output reads the
/// dense id array positionally, anything else parses the row value with the
/// historical seed-slot priority. Returns `None` for unparseable seeds.
pub(super) fn seed_vid(
    identity_seeds: Option<&[VertexId]>,
    index: usize,
    row: &[Value],
    seed_slot: usize,
) -> Option<VertexId> {
    if let Some(ids) = identity_seeds {
        return Some(ids[index]);
    }
    let vid_val = row
        .get(seed_slot)
        .or_else(|| row.first())
        .cloned()
        .unwrap_or(Value::Null(linkrs_core::NullType::Null));
    VertexId::try_from(&vid_val).ok()
}

pub(super) fn row_passes_filter(
    row: &[Value],
    col_names: &[String],
    filter: &Option<Expression>,
) -> bool {
    let Some(expr) = filter else {
        return true;
    };

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut context = ValueRowContext::new(row.to_vec(), layout);
    matches!(
        ExpressionEvaluator::evaluate(expr, &mut context),
        Ok(Value::Bool(true))
    )
}

/// Pre-resolve the seed-variable slot for the fast expand paths.  Mirrors the
/// historical extraction priority (`"vid"`, `"src"`, the first col-name
/// template entry, then column 0) without building a per-row expression
/// context.
pub(super) fn seed_slot(layout: &SlotLayout, col_names_template: &[String]) -> usize {
    if let Some(slot) = layout.slot_id("vid") {
        return slot;
    }
    if let Some(slot) = layout.slot_id("src") {
        return slot;
    }
    if let Some(name) = col_names_template.first() {
        if let Some(slot) = layout.slot_id(name) {
            return slot;
        }
    }
    0
}

/// Forward a seed row into an id_only expand output with the source column
/// replaced by a lightweight `Value::VertexId`, so intermediate hops never
/// deep-clone the full `Value::Vertex(Box)` (with its property maps) that a
/// storage scan puts in the entity column.
pub(super) fn lightweight_seed_row(row: &[Value], src_slot: usize, vid: VertexId) -> Vec<Value> {
    if matches!(row.get(src_slot), Some(Value::VertexId(_))) {
        return row.to_vec();
    }
    let mut out = Vec::with_capacity(row.len());
    for (i, val) in row.iter().enumerate() {
        if i == src_slot {
            out.push(Value::VertexId(vid));
        } else {
            out.push(val.clone());
        }
    }
    out
}

/// Parse seeds from both row and rowless inputs. Returns the seed ids, the
/// materialized seed rows for row outputs, and the absolute input positions
/// for typed gathering in rowless outputs.
pub(super) fn parse_seeds(
    chunk: &DataChunk,
    seed_slot: usize,
    src_vids: &[Value],
) -> (Vec<VertexId>, Vec<Vec<Value>>, Vec<usize>) {
    let mut vids = Vec::new();
    let mut rows = Vec::new();
    let mut positions = Vec::new();
    let identity = identity_seed_ids(chunk, seed_slot);
    if !chunk.rows.is_empty() {
        for (index, row) in visible_rows(chunk) {
            if let Some(vid) = seed_vid(identity, index, row, seed_slot) {
                vids.push(vid);
                rows.push(row.clone());
                positions.push(index);
            }
        }
    } else if let Some(n) = chunk.typed_len() {
        let selected: Vec<usize> = chunk
            .selection()
            .map(|s| s.to_vec())
            .unwrap_or_else(|| (0..n).collect());
        let width = chunk.get_layout().len();
        for pos in selected {
            let vid_opt = if let Some(ids) = identity {
                ids.get(pos).copied()
            } else {
                chunk
                    .get_typed_by_slot(pos, seed_slot)
                    .and_then(|v| VertexId::try_from(&v).ok())
            };
            if let Some(vid) = vid_opt {
                vids.push(vid);
                let mut row = Vec::with_capacity(width);
                for slot in 0..width {
                    row.push(
                        chunk
                            .get_typed_by_slot(pos, slot)
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
                    );
                }
                rows.push(row);
                positions.push(pos);
            }
        }
    }
    if vids.is_empty() && !src_vids.is_empty() {
        for vid_val in src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                vids.push(vid);
                rows.push(Vec::new());
                positions.push(0);
            }
        }
    }
    (vids, rows, positions)
}

/// Unified row-materialization boundary for row-path expand operators.
///
/// Rowless chunks carry data only in typed columns; a row-path loop over
/// `chunk.rows` would observe zero seeds and silently drop the input (e.g. an
/// upstream closed-loop hop feeding a downstream hop that falls back to the
/// row path on open schemas, filters or path semantics). Every row-path entry
/// point must call this first: rowless inputs rebuild into a compact row view
/// with the selection applied and the typed layout gathered in lockstep, row
/// inputs pass through untouched.
pub(super) fn materialize_rowless_rows(mut chunk: DataChunk) -> DataChunk {
    if !chunk.rows.is_empty() || chunk.typed_columns.is_none() {
        return chunk;
    }
    let Some(n) = chunk.typed_len() else {
        return chunk;
    };
    let selected: Vec<usize> = chunk
        .selection()
        .map(|s| s.to_vec())
        .unwrap_or_else(|| (0..n).collect());
    let width = chunk.get_layout().len();
    let mut rows = Vec::with_capacity(selected.len());
    for pos in &selected {
        let mut row = Vec::with_capacity(width);
        for slot in 0..width {
            row.push(
                chunk
                    .get_typed_by_slot(*pos, slot)
                    .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
            );
        }
        rows.push(row);
    }
    if let Some(cols) = chunk.typed_columns.take() {
        chunk.typed_columns = Some(
            cols.iter()
                .map(|col| gather_typed_column(col, &selected))
                .collect(),
        );
    }
    chunk.rows = rows;
    chunk.selection = None;
    chunk
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_of(names: &[&str]) -> SlotLayout {
        SlotLayout::from_names(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn materialize_rowless_rebuilds_visible_rows_in_lockstep() {
        let layout = Arc::new(layout_of(&["a", "b"]));
        let mut chunk = DataChunk::new_with_layout(Vec::new(), Arc::clone(&layout));
        chunk.typed_columns = Some(vec![
            TypedColumn::Fallback(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
            TypedColumn::Fallback(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
        ]);
        chunk.selection = Some(vec![0, 2]);
        let out = materialize_rowless_rows(chunk);
        assert_eq!(
            out.rows,
            vec![
                vec![Value::Int(1), Value::Int(10)],
                vec![Value::Int(3), Value::Int(30)]
            ],
            "only visible positions materialize"
        );
        assert!(
            out.selection().is_none(),
            "materialization consumes the selection"
        );
        assert_eq!(
            out.typed_len(),
            Some(2),
            "typed layout stays in lockstep with the rebuilt rows"
        );
    }

    #[test]
    fn materialize_rowless_passes_rows_through() {
        let layout = Arc::new(layout_of(&["a"]));
        let chunk = DataChunk::new_with_layout(vec![vec![Value::Int(7)]], layout);
        let out = materialize_rowless_rows(chunk);
        assert_eq!(out.rows, vec![vec![Value::Int(7)]]);
    }
}

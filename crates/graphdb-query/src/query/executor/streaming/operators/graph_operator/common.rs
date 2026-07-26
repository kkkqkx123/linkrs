use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::slot::SlotLayout;
    use crate::query::executor::traversal::config::TraversalConfig;
    use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
    use crate::query::executor::traversal::runtime::TraversalRuntime;
use crate::storage::QueryStorage;

use super::super::visited_set::VisitedSet;

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

pub(super) fn expand_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    col_names_template: Vec<String>,
    src_vids: Vec<Value>,
    cancel_token: Option<Arc<AtomicBool>>,
    step_limit: u32,
) -> Result<Option<DataChunk>, QueryError> {
    let _chunk_col_names = chunk.col_names();

    // Build the list of seed vertex IDs: from the chunk rows, or from explicit src_vids.
    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();

    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let src_name = col_names_template.first().map(|s| s.as_str());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| context.get_variable("src"))
            .or_else(|| src_name.and_then(|name| context.get_variable(name)))
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            seed_rows.push(row.clone());
        }
    }

    // If no valid vids came from the input chunk but src_vids are provided, use those.
    if seed_vids.is_empty() && !src_vids.is_empty() {
        for vid_val in &src_vids {
            if let Ok(vid) = VertexId::try_from(vid_val) {
                seed_vids.push(vid);
                seed_rows.push(Vec::new());
            }
        }
    }

    let mut out_rows = Vec::new();
    for (vid, row) in seed_vids.iter().zip(seed_rows.iter()) {
        let config = if step_limit > 1 {
            TraversalConfig {
                min_depth: step_limit,
                max_depth: step_limit,
                ..TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec())
            }
        } else {
            TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec())
        };
        let runtime_reader = TraversalGraphReader::new(reader);
        let mut runtime = TraversalRuntime::new(runtime_reader, config);
        if let Some(token) = cancel_token.clone() {
            runtime.set_cancel_token(token);
        }

        if let Ok(Some(vertex)) = reader.get_vertex(space_name, vid) {
            runtime.seed_from_vertex(vertex);
        } else {
            continue;
        }

        while let Some(event) = runtime.next_event() {
            let mut out_row = row.clone();
            if let Some(ref edge) = event.edge {
                out_row.push(Value::Edge(Box::new(edge.clone())));
            } else {
                out_row.push(Value::Null(crate::core::NullType::Null));
            }
            out_row.push(Value::Vertex(Box::new(event.vertex)));
            let mut out_col_names = col_names_template.clone();
            out_col_names.push("_expand_edge".to_string());
            out_col_names.push("_expand_dst".to_string());
            if row_passes_filter(&out_row, &out_col_names, filter_expr) {
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

pub(super) fn traverse_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let _col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));
        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config.clone());
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(&config.space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let nid = event.vertex.vid();
                if !visited.insert(*nid) {
                    continue;
                }

                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::string(edge_type));
                out_row.push(Value::string(dir_str));
                out_row.push(Value::BigInt(event.depth as i64));
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

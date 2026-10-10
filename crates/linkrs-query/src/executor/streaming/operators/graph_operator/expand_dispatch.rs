use std::sync::Arc;

use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::streaming::chunk::{DataChunk, TypedColumn};
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::traversal::config::TraversalConfig;
use crate::executor::traversal::graph_reader::TraversalGraphReader;
use crate::executor::traversal::runtime::TraversalRuntime;
use crate::parser::ast::pattern::PathSemantic;
use crate::storage::QueryStorage;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::{Edge, EdgeDirection, Value, Vertex};

use super::super::visited_set::VisitedSet;
use super::expand_buffer::visible_rows;
use super::expand_columnar::check_output_layout;
use super::expand_seeds::{materialize_rowless_rows, row_passes_filter, seed_slot};
use super::ExpandCtx;

/// Columnar expand outcome with an explicit decline signal.
///
/// Produced carries the output chunk, Empty confirms the input chunk has no
/// expansion rows, Declined means the columnar path does not serve this shape
/// and the caller must fall back to the row path instead of dropping input.
pub(super) enum ColumnarOutcome {
    Produced(DataChunk),
    Empty,
    Declined,
}

/// Verify the planner closed-loop claim against storage schemas. Every edge
/// type must declare endpoint labels and the planned `dst_tag` must match the
/// neighbor side. An empty plan tag is allowed when all edge types agree on
/// one neighbor label, which the executor then derives from schema.
/// Anything else falls back to the row path.
pub(super) fn is_closed_loop_storage(
    reader: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    dst_tag: &str,
) -> bool {
    closed_loop_dst_tag(reader, space_name, edge_types, direction, dst_tag).is_some()
}

/// The single agreed neighbor label for a closed loop, or `None` when the
/// schemas disagree or are untyped. An empty plan tag falls back to the schema
/// label so anonymous endpoints stay closed-loop.
pub(super) fn closed_loop_dst_tag(
    reader: &dyn QueryStorage,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    dst_tag: &str,
) -> Option<String> {
    if edge_types.is_empty() {
        return None;
    }
    let mut agreed: Option<String> = None;
    for edge_type in edge_types {
        let Ok(Some(info)) = reader.get_edge_type(space_name, edge_type) else {
            return None;
        };
        if info.src_tag_name.is_empty() || info.dst_tag_name.is_empty() {
            return None;
        }
        let neighbor = match direction {
            EdgeDirection::Out => info.dst_tag_name.clone(),
            EdgeDirection::In => info.src_tag_name.clone(),
            EdgeDirection::Both => {
                if info.src_tag_name != info.dst_tag_name {
                    return None;
                }
                info.src_tag_name.clone()
            }
        };
        if !dst_tag.is_empty() && neighbor != dst_tag {
            return None;
        }
        match agreed.as_ref() {
            None => agreed = Some(neighbor),
            Some(prev) if prev == &neighbor => {}
            _ => return None,
        }
    }
    agreed
}

/// Convert the planner edge demand into a storage projection. `None` means the
/// whole edge (all properties), `Some([])` means topology only.
pub(super) fn edge_projection(edge_required: Option<&Vec<String>>) -> Option<Vec<std::sync::Arc<str>>> {
    match edge_required {
        None => None,
        Some(props) => Some(props.iter().map(|s| std::sync::Arc::from(s.as_str())).collect()),
    }
}

/// Whether the hop demands a flat bypass name already present in the input
/// prefix. A conflict means the current hop rebinds a variable whose old
/// bypass column is still carried upstream: reusing that slot would point the
/// compound read at the old entity, so the hop must not use the flat bypass
/// contract and stays on the row path.
pub(super) fn has_bypass_conflict(
    col_names: &[String],
    edge_props: Option<&Vec<String>>,
    dst_props: Option<&Vec<String>>,
    input: &SlotLayout,
) -> bool {
    let (Some(edge_var), Some(dst_var)) = (col_names.get(1), col_names.get(2)) else {
        return false;
    };
    if let Some(props) = edge_props {
        for prop in props {
            if input.slot_id(&format!("{edge_var}.{prop}")).is_some() {
                return true;
            }
        }
    }
    if let Some(props) = dst_props {
        for prop in props {
            if input.slot_id(&format!("{dst_var}.{prop}")).is_some() {
                return true;
            }
        }
    }
    false
}

/// New flat bypass properties for this hop, excluding columns already present
/// in the input prefix (upstream bypass passthrough).
///
/// Returns `None` per slot when the slot needs the whole entity: the hop must
/// stay on the row path with boxed values. `Some` (possibly empty) lists the
/// demanded properties in deterministic sorted order, matching the plan
/// layout's bypass suffix. Malformed column templates also yield `None` so
/// the row path keeps entity extraction working. Callers gate rowless on
/// [`has_bypass_conflict`]: filtered assembly still matches the layout for
/// the row path, but a conflicting hop never takes the rowless contract.
pub(super) fn hop_bypass(
    col_names: &[String],
    edge_props: Option<&Vec<String>>,
    dst_props: Option<&Vec<String>>,
    input: &SlotLayout,
) -> (Option<Vec<String>>, Option<Vec<String>>) {
    let (Some(edge_var), Some(dst_var)) = (col_names.get(1), col_names.get(2)) else {
        return (None, None);
    };
    let mut taken: Vec<String> = Vec::new();
    let edge = match edge_props {
        None => None,
        Some(props) => {
            let mut sorted = props.clone();
            sorted.sort();
            sorted.dedup();
            let mut out = Vec::new();
            for prop in sorted {
                let name = format!("{edge_var}.{prop}");
                if input.slot_id(&name).is_none() && !taken.contains(&name) {
                    taken.push(name);
                    out.push(prop);
                }
            }
            Some(out)
        }
    };
    let dst = match dst_props {
        None => None,
        Some(props) => {
            let mut sorted = props.clone();
            sorted.sort();
            sorted.dedup();
            let mut out = Vec::new();
            for prop in sorted {
                let name = format!("{dst_var}.{prop}");
                if input.slot_id(&name).is_none() && !taken.contains(&name) {
                    taken.push(name);
                    out.push(prop);
                }
            }
            Some(out)
        }
    };
    (edge, dst)
}

/// Edge bypass value: the demanded property or NULL when the edge lacks it,
/// matching row-path `Edge` property reads.
pub(super) fn edge_bypass_value(edge: &Edge, prop: &str) -> Value {
    edge.props
        .get(prop)
        .cloned()
        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
}

/// Destination bypass value: the demanded property or NULL when the vertex
/// lacks it, matching row-path `Vertex` property reads.
pub(super) fn dst_bypass_value(vertex: Option<&Vertex>, prop: &str) -> Value {
    vertex
        .and_then(|v| v.property_value(prop))
        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
}

/// Bypass columns stay `Fallback`: mixed property kinds keep exact `Value`
/// semantics, and the direct-property consumer reads them through the
/// compound-slot column path.
pub(super) fn bypass_column(values: Vec<Value>) -> TypedColumn {
    TypedColumn::Fallback(values)
}

pub(super) fn expand_on_chunk(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    step_limit: u32,
    ctx: &mut ExpandCtx,
) -> Result<Option<DataChunk>, QueryError> {
    let chunk = materialize_rowless_rows(chunk);
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let filter_expr = ctx.filter_expr;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);

    // Build the list of seed vertex IDs: from the chunk rows, or from explicit src_vids.
    // Seeds prefer full vertex values (preserving tags); bare ids without
    // tags are illegal under single-label semantics.
    let mut seed_vids: Vec<VertexId> = Vec::new();
    let mut seed_rows: Vec<Vec<Value>> = Vec::new();
    let mut seed_vertices: Vec<Option<linkrs_core::Vertex>> = Vec::new();

    for (_, row) in visible_rows(&chunk) {
        let vid_val = row
            .get(seed_slot)
            .or_else(|| row.first())
            .cloned()
            .unwrap_or(Value::Null(linkrs_core::NullType::Null));

        if let Value::Vertex(vertex) = &vid_val {
            seed_vids.push(vertex.vid);
            seed_rows.push(row.clone());
            seed_vertices.push(Some((**vertex).clone()));
        } else if let Ok(vid) = VertexId::try_from(&vid_val) {
            seed_vids.push(vid);
            seed_rows.push(row.clone());
            seed_vertices.push(None);
        }
    }

    // Rowless identity seeds carry only `VertexId`: rebuild full seeds from
    // the hop edge schemas before the row walk. A unique seed-side label
    // batch-reads vertices; ambiguous or unresolvable domains error instead
    // of guessing a label. Missing seeds walk nothing.
    if seed_vertices.iter().any(|v| v.is_none()) && !seed_vids.is_empty() {
        let seed_tag = crate::executor::traversal::graph_reader::resolve_seed_tag(
            reader,
            space_name,
            edge_types,
            direction,
            "",
        )?;
        let mut bare_ids: Vec<VertexId> = Vec::new();
        let mut bare_pos: Vec<usize> = Vec::new();
        for (idx, vertex) in seed_vertices.iter().enumerate() {
            if vertex.is_none() {
                bare_ids.push(seed_vids[idx]);
                bare_pos.push(idx);
            }
        }
        if !bare_ids.is_empty() {
            let fetched = reader.get_vertices_batch(space_name, &seed_tag, &bare_ids)?;
            let mut missing: Vec<usize> = Vec::new();
            for (pos, vertex) in bare_pos.iter().zip(fetched.into_iter()) {
                match vertex {
                    Some(v) => {
                        seed_vertices[*pos] = Some(v.clone());
                        if let Some(slot) = seed_rows.get_mut(*pos).and_then(|r| r.get_mut(seed_slot))
                        {
                            *slot = Value::Vertex(Box::new(v));
                        }
                    }
                    None => missing.push(*pos),
                }
            }
            if !missing.is_empty() {
                let drop: std::collections::HashSet<usize> =
                    missing.into_iter().collect();
                let mut kept_vids = Vec::with_capacity(seed_vids.len() - drop.len());
                let mut kept_rows = Vec::with_capacity(seed_rows.len() - drop.len());
                let mut kept_vertices =
                    Vec::with_capacity(seed_vertices.len() - drop.len());
                for (idx, ((vid, row), vertex)) in seed_vids
                    .into_iter()
                    .zip(seed_rows.into_iter())
                    .zip(seed_vertices.into_iter())
                    .enumerate()
                {
                    if !drop.contains(&idx) {
                        kept_vids.push(vid);
                        kept_rows.push(row);
                        kept_vertices.push(vertex);
                    }
                }
                seed_vids = kept_vids;
                seed_rows = kept_rows;
                seed_vertices = kept_vertices;
            }
        }
    }

    // Literal seeds carry no label: resolve them in the seed domain
    // determined by the edge schemas. Seeds missing from that domain walk
    // nothing; ambiguous domains error instead of guessing a label.
    if seed_vids.is_empty() && !src_vids.is_empty() {
        let scope_tag = if src_vids.iter().any(|v| !matches!(v, Value::Vertex(_))) {
            Some(crate::executor::traversal::graph_reader::resolve_seed_tag(
                reader,
                space_name,
                edge_types,
                direction,
                ctx.dst_tag,
            )?)
        } else {
            None
        };
        for vid_val in &src_vids {
            if let Value::Vertex(vertex) = vid_val {
                seed_vids.push(vertex.vid);
                seed_rows.push(Vec::new());
                seed_vertices.push(Some((**vertex).clone()));
            } else if let Ok(vid) = VertexId::try_from(vid_val) {
                let Some(scope) = scope_tag.as_deref() else {
                    continue;
                };
                let Some(vertex) = reader.get_vertex(space_name, scope, &vid)? else {
                    continue;
                };
                seed_vids.push(vid);
                seed_rows.push(Vec::new());
                seed_vertices.push(Some(vertex));
            }
        }
    }

    let (edge_bypass_opt, dst_bypass_opt) = hop_bypass(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &chunk.get_layout(),
    );
    let edge_bypass = edge_bypass_opt.unwrap_or_default();
    let dst_bypass = dst_bypass_opt.unwrap_or_default();

    let mut out_rows = Vec::new();
    for ((vid, row), seed_vertex) in seed_vids
        .iter()
        .zip(seed_rows.iter())
        .zip(seed_vertices.iter())
    {
        let _ = vid;
        let mut config =
            TraversalConfig::expand(space_name.to_string(), direction, edge_types.to_vec());
        if step_limit > 1 {
            config.min_depth = step_limit;
            config.max_depth = step_limit;
        }
        config.vertex_tag = ctx.dst_tag.to_string();
        config.path_semantic = ctx.path_semantic.clone();
        match config.path_semantic {
            // Walk/Trail/Acyclic allow or constrain repeats per path, so
            // the runtime must not apply global vertex dedup. Trail and
            // Acyclic are enforced per path inside TraversalRuntime.
            Some(PathSemantic::Walk)
            | Some(PathSemantic::Trail)
            | Some(PathSemantic::Acyclic)
            | None => {
                if config.path_semantic.is_some() {
                    config.visited_policy = crate::executor::traversal::config::VisitedPolicy::None;
                }
            }
            // Shortest variants rely on a global first-visit order: the
            // first time a vertex is reached is via a shortest path, so
            // global dedup is the algorithm rather than an optimization.
            // The weighted variant runs Dijkstra inside the runtime, which
            // also needs global dedup.
            Some(PathSemantic::Shortest)
            | Some(PathSemantic::AllShortest)
            | Some(PathSemantic::WeightedShortest(_)) => {
                config.visited_policy = crate::executor::traversal::config::VisitedPolicy::Global;
                config.order = crate::executor::traversal::config::TraversalOrder::Bfs;
            }
        }
        let runtime_reader = TraversalGraphReader::new(reader);
        let mut runtime = TraversalRuntime::new(runtime_reader, config);
        if let Some(token) = ctx.cancel_token.clone() {
            runtime.set_cancel_token(token);
        }

        if let Some(vertex) = seed_vertex.clone() {
            runtime.seed_from_vertex(vertex);
        } else {
            return Err(QueryError::execution(
                "Traversal seed requires a vertex value with tag; bare id is illegal".to_string(),
            ));
        }

        while let Some(event) = runtime.next_event() {
            let mut out_row = row.clone();
            if let Some(ref edge) = event.edge {
                out_row.push(Value::Edge(Box::new(edge.clone())));
            } else {
                out_row.push(Value::Null(linkrs_core::NullType::Null));
            }
            out_row.push(Value::Vertex(Box::new(event.vertex.clone())));
            for prop in &edge_bypass {
                let value = match event.edge.as_ref() {
                    Some(edge) => edge_bypass_value(edge, prop),
                    None => Value::Null(linkrs_core::NullType::Null),
                };
                out_row.push(value);
            }
            for prop in &dst_bypass {
                out_row.push(dst_bypass_value(Some(&event.vertex), prop));
            }
            let mut out_col_names = ctx.col_names_template.clone();
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
    check_output_layout(
        &output_layout,
        output_layout.len(),
        out_rows.first().map(Vec::len),
    )?;

    Ok(Some(DataChunk::new_with_layout(out_rows, output_layout)))
}

pub(super) fn traverse_on_chunk_with_semantic(
    chunk: DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    skip_visited: bool,
    cancel_token: Option<CancelToken>,
) -> Result<Option<DataChunk>, QueryError> {
    let _col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for (_, row) in visible_rows(&chunk) {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(linkrs_core::NullType::Null));
        let seed_vertex = match &vid_val {
            Value::Vertex(vertex) => Some((**vertex).clone()),
            _ => None,
        };
        let Some(seed) = seed_vertex else {
            return Err(QueryError::execution(
                "Traversal seed requires a vertex value with tag; bare id is illegal".to_string(),
            ));
        };
        {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime_config = config.clone();
            // Keep the declared semantic for the runtime: Trail/Acyclic
            // are enforced per path, Shortest uses BFS first-visit and
            // WeightedShortest uses Dijkstra (both need global dedup).
            // The operator-level `skip_visited` (global VisitedSet) is
            // applied separately below so per-path semantics are never
            // silently replaced by global dedup.
            match runtime_config.path_semantic {
                Some(PathSemantic::Walk)
                | Some(PathSemantic::Trail)
                | Some(PathSemantic::Acyclic)
                | None => {
                    if runtime_config.path_semantic.is_some() {
                        runtime_config.visited_policy =
                            crate::executor::traversal::config::VisitedPolicy::None;
                    }
                }
                Some(PathSemantic::Shortest)
                | Some(PathSemantic::AllShortest)
                | Some(PathSemantic::WeightedShortest(_)) => {
                    runtime_config.visited_policy =
                        crate::executor::traversal::config::VisitedPolicy::Global;
                    runtime_config.order = crate::executor::traversal::config::TraversalOrder::Bfs;
                }
            }
            let mut runtime = TraversalRuntime::new(runtime_reader, runtime_config);
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            runtime.seed_from_vertex(seed);

            while let Some(event) = runtime.next_event() {
                let nid = event.vertex.vid();
                if skip_visited && !visited.insert(*nid) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn layout_of(names: &[&str]) -> SlotLayout {
        SlotLayout::from_names(&names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn col_names_of(vars: &[&str]) -> Vec<String> {
        vars.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn hop_bypass_sorts_and_filters_input_prefix() {
        let input = layout_of(&["a", "b.name"]);
        let (edge, dst) = hop_bypass(
            &col_names_of(&["a", "r", "b"]),
            Some(&vec!["weight".to_string(), "kind".to_string()]),
            Some(&vec!["name".to_string(), "age".to_string()]),
            &input,
        );
        assert_eq!(
            edge,
            Some(vec!["kind".to_string(), "weight".to_string()]),
            "edge demands stay sorted"
        );
        assert_eq!(
            dst,
            Some(vec!["age".to_string()]),
            "upstream bypass columns are not duplicated"
        );
    }

    #[test]
    fn hop_bypass_full_entity_forces_row_path() {
        let input = layout_of(&["a"]);
        let (edge, dst) = hop_bypass(
            &col_names_of(&["a", "r", "b"]),
            None,
            Some(&vec!["name".to_string()]),
            &input,
        );
        assert_eq!(edge, None, "whole-edge use must stay on the row path");
        assert!(dst.is_some(), "destination bypass alone still resolves");
    }
}

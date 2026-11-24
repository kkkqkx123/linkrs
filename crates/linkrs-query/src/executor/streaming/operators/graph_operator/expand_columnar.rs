use std::sync::Arc;

use crate::executor::streaming::chunk::{
    gather_typed_column, DataChunk, EdgeHeaderColumn, PackedExpandPayload, TypedColumn,
};
use crate::executor::streaming::runtime::ColumnarStats;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::{EdgeTopology, PackedEdgeTopology, QueryStorage};
use linkrs_core::error::QueryError;
use linkrs_core::types::{storage_ids::VertexId, LabelId};
use linkrs_core::{Edge, EdgeDirection, EdgeHeader, Value, Vertex};

use super::expand_dispatch::{
    bypass_column, edge_bypass_value, edge_projection, has_bypass_conflict, hop_bypass,
    ColumnarOutcome,
};
use super::expand_seeds::{identity_seed_handles, parse_seeds, seed_slot};
use super::ExpandCtx;

/// Destination batch read honoring the bypass projection.
///
/// Flat demand uses property columns (or existence only) so no entity value
/// is built; whole-entity demand keeps the full vertex batch. Both views
/// preserve the batch timestamp, input order and visibility, with one entry
/// per id and missing vertices marked absent so dangling edges drop the row.
pub(super) enum DestinationView {
    Vertices {
        values: Vec<Option<Vertex>>,
        slots: Vec<usize>,
    },
    Properties {
        names: Vec<String>,
        present: Vec<bool>,
        values: Vec<Vec<Value>>,
        slots: Vec<usize>,
    },
}

impl DestinationView {
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Vertices { slots, .. } | Self::Properties { slots, .. } => slots.len(),
        }
    }

    fn slot(&self, index: usize) -> Option<usize> {
        match self {
            Self::Vertices { slots, .. } | Self::Properties { slots, .. } => {
                slots.get(index).copied()
            }
        }
    }

    pub(super) fn present(&self, index: usize) -> bool {
        match self {
            Self::Vertices { values, .. } => self
                .slot(index)
                .and_then(|slot| values.get(slot))
                .is_some_and(Option::is_some),
            Self::Properties { present, .. } => self
                .slot(index)
                .and_then(|slot| present.get(slot))
                .copied()
                .unwrap_or(false),
        }
    }

    pub(super) fn vertex(&self, index: usize) -> Option<&Vertex> {
        match self {
            Self::Vertices { values, .. } => self
                .slot(index)
                .and_then(|slot| values.get(slot))
                .and_then(|value| value.as_ref()),
            Self::Properties { .. } => None,
        }
    }

    pub(super) fn property(&self, index: usize, name: &str) -> Value {
        let slot = self.slot(index);
        match self {
            Self::Vertices { values, .. } => slot
                .and_then(|slot| values.get(slot))
                .and_then(|value| value.as_ref())
                .and_then(|value| value.property_value(name))
                .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
            Self::Properties { names, values, .. } => names
                .iter()
                .position(|item| item == name)
                .and_then(|column| slot.and_then(|slot| values.get(column)?.get(slot).cloned()))
                .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
        }
    }
}

/// Destination read result with its storage-only and view-only costs kept
/// separate for EXPLAIN attribution.
pub(super) struct DestinationRead {
    pub(super) view: DestinationView,
    pub(super) read_us: u64,
    pub(super) view_us: u64,
    pub(super) unique: usize,
    pub(super) missing: usize,
}

pub(super) fn read_destinations(
    reader: &dyn QueryStorage,
    space_name: &str,
    dst_tag: &str,
    ids: &[VertexId],
    dst_props: Option<&Vec<String>>,
    property_columns_only: bool,
    handles: Option<&[Option<(u32, LabelId)>]>,
) -> Result<DestinationRead, QueryError> {
    // A hop emits one row per edge, but many edges share a destination.
    // Point lookups at one snapshot are pure, so fetch each distinct id once
    // and restore input order for the row assembly.
    let dedup_started = std::time::Instant::now();
    let mut unique = Vec::with_capacity(ids.len());
    let mut unique_handles: Vec<Option<(u32, LabelId)>> = Vec::with_capacity(ids.len());
    let mut slots = Vec::with_capacity(ids.len());
    let mut index = std::collections::HashMap::new();
    for (edge, id) in ids.iter().enumerate() {
        let slot = if let Some(&slot) = index.get(id) {
            slot
        } else {
            let slot = unique.len();
            index.insert(*id, slot);
            unique.push(*id);
            unique_handles.push(handles.and_then(|h| h.get(edge)).copied().flatten());
            slot
        };
        slots.push(slot);
    }
    let mut view_us = dedup_started.elapsed().as_micros() as u64;
    let unique_count = unique.len();

    if property_columns_only {
        if let Some(props) = dst_props {
            let mut names = props.clone();
            names.sort();
            names.dedup();
            let projection: Vec<std::sync::Arc<str>> = names
                .iter()
                .map(|s| std::sync::Arc::from(s.as_str()))
                .collect();
            let read_started = std::time::Instant::now();
            let columns = if handles.is_some() {
                reader.get_vertex_property_columns_with_handles_batch(
                    space_name,
                    dst_tag,
                    &unique,
                    &projection,
                    &unique_handles,
                )?
            } else {
                reader.get_vertex_property_columns_batch(
                    space_name,
                    dst_tag,
                    &unique,
                    &projection,
                )?
            };
            let read_us = read_started.elapsed().as_micros() as u64;
            let present: Vec<bool> = columns.iter().map(Option::is_some).collect();
            let missing = present.iter().filter(|found| !**found).count();
            let expansion_started = std::time::Instant::now();
            let mut values: Vec<Vec<Value>> = vec![Vec::with_capacity(unique.len()); names.len()];
            for column in columns {
                let row = column
                    .unwrap_or_else(|| vec![Value::Null(linkrs_core::NullType::Null); names.len()]);
                for (slot, value) in row.into_iter().enumerate() {
                    values[slot].push(value);
                }
            }
            view_us += expansion_started.elapsed().as_micros() as u64;
            return Ok(DestinationRead {
                view: DestinationView::Properties {
                    names,
                    present,
                    values,
                    slots,
                },
                read_us,
                view_us,
                unique: unique_count,
                missing,
            });
        }
    }

    let read_started = std::time::Instant::now();
    let vertices = match dst_props {
        Some(props) if !props.is_empty() => {
            let mut sorted = props.clone();
            sorted.sort();
            sorted.dedup();
            let projection: Vec<std::sync::Arc<str>> = sorted
                .iter()
                .map(|s| std::sync::Arc::from(s.as_str()))
                .collect();
            reader.get_vertices_projected_batch(space_name, dst_tag, &unique, &projection)?
        }
        Some(_) => reader.get_vertices_projected_batch(space_name, dst_tag, &unique, &[])?,
        None => reader.get_vertices_batch(space_name, dst_tag, &unique)?,
    };
    let read_us = read_started.elapsed().as_micros() as u64;
    let missing = vertices.iter().filter(|vertex| vertex.is_none()).count();
    let view = DestinationView::Vertices {
        values: vertices,
        slots,
    };
    Ok(DestinationRead {
        view,
        read_us,
        view_us,
        unique: unique_count,
        missing,
    })
}

/// Explicit layout lockstep check: typed arms and row width must match the
/// plan layout exactly instead of silently misaligning.
pub(super) fn check_output_layout(
    output_layout: &SlotLayout,
    typed_arms: usize,
    row_width: Option<usize>,
) -> Result<(), QueryError> {
    if typed_arms != output_layout.len() {
        return Err(QueryError::execution(format!(
            "expand typed arms {} do not match layout width {}",
            typed_arms,
            output_layout.len()
        )));
    }
    if let Some(width) = row_width {
        if width != output_layout.len() {
            return Err(QueryError::execution(format!(
                "expand row width {} does not match layout width {}",
                width,
                output_layout.len()
            )));
        }
    }
    Ok(())
}

/// Runtime lockstep check for rowless construction: every typed arm must
/// carry exactly `retained` rows and the arm count must match the layout.
/// Replaces scattered debug assertions so release builds fail explicitly
/// instead of misaligning downstream consumers.
pub(super) fn check_rowless_lockstep(
    output_layout: &SlotLayout,
    typed: &[TypedColumn],
    retained: usize,
) -> Result<(), QueryError> {
    if typed.len() != output_layout.len() {
        return Err(QueryError::execution(format!(
            "expand typed arms {} do not match layout width {}",
            typed.len(),
            output_layout.len()
        )));
    }
    for (idx, col) in typed.iter().enumerate() {
        if col.len() != retained {
            return Err(QueryError::execution(format!(
                "expand typed arm {idx} length {} does not match retained rows {retained}",
                col.len()
            )));
        }
    }
    Ok(())
}

/// Runtime lockstep check for row outputs that also carry typed arms: row
/// width, typed arm count and every arm length must agree with the layout
/// and the retained row count.
pub(super) fn check_row_lockstep(
    output_layout: &SlotLayout,
    typed: &[TypedColumn],
    rows: &[Vec<Value>],
) -> Result<(), QueryError> {
    let retained = rows.len();
    let width = rows.first().map(Vec::len);
    check_output_layout(output_layout, typed.len(), width)?;
    for (idx, col) in typed.iter().enumerate() {
        if col.len() != retained {
            return Err(QueryError::execution(format!(
                "expand typed arm {idx} length {} does not match retained rows {retained}",
                col.len()
            )));
        }
    }
    Ok(())
}

/// Destination id on one side of an edge for the hop direction.
pub(super) fn edge_neighbor(edge: &Edge, seed: &VertexId, direction: EdgeDirection) -> VertexId {
    match direction {
        EdgeDirection::Out => *edge.dst(),
        EdgeDirection::In => *edge.src(),
        EdgeDirection::Both => {
            if edge.src() == seed {
                *edge.dst()
            } else {
                *edge.src()
            }
        }
    }
}

/// Project a packed endpoint onto the declared primary-key value type.
fn identity_key_value(vid: VertexId, data_type: &linkrs_core::DataType) -> Option<Value> {
    let id = vid.as_int64()?;
    match data_type {
        linkrs_core::DataType::BigInt => Some(Value::BigInt(id)),
        linkrs_core::DataType::Int => i32::try_from(id).ok().map(Value::Int),
        linkrs_core::DataType::SmallInt => i16::try_from(id).ok().map(Value::SmallInt),
        _ => None,
    }
}

/// Identity-key fast path for one packed expand batch.
///
/// The plan proves that the demanded destination property is the declared
/// primary key, so the endpoint id from the topology scan is the key itself.
/// Destinations are still checked for snapshot visibility, but their property
/// columns are not read and no per-edge key array is built. `None` declines
/// the fast path so the ordinary typed assembly remains authoritative.
fn expand_packed_identity(
    reader: &dyn crate::storage::QueryStorage,
    space_name: &str,
    dst_tag: &str,
    packed: &PackedEdgeTopology,
    direction: linkrs_core::EdgeDirection,
    key_name: &str,
    identity_type: &linkrs_core::DataType,
    seed_count: usize,
    stats: Option<&ColumnarStats>,
    output_layout: Arc<crate::executor::streaming::slot::SlotLayout>,
) -> Result<Option<ColumnarOutcome>, QueryError> {
    use linkrs_core::EdgeDirection;

    let far_ids: &[VertexId] = match direction {
        EdgeDirection::Out => &packed.dst,
        EdgeDirection::In => &packed.src,
        _ => return Ok(None),
    };
    if far_ids.len() != packed.destination_handles.len() {
        return Ok(None);
    }
    let mut unique = Vec::with_capacity(far_ids.len());
    let mut handles = Vec::with_capacity(far_ids.len());
    let mut weights: Vec<u64> = Vec::with_capacity(far_ids.len());
    let mut slots: std::collections::HashMap<VertexId, usize> = std::collections::HashMap::new();
    for (id, handle) in far_ids.iter().zip(&packed.destination_handles) {
        let slot = match slots.get(id) {
            Some(&slot) => slot,
            None => {
                let slot = unique.len();
                slots.insert(*id, slot);
                unique.push(*id);
                handles.push(*handle);
                weights.push(0);
                slot
            }
        };
        weights[slot] += 1;
    }
    let Some(keys) = unique
        .iter()
        .map(|id| identity_key_value(*id, identity_type))
        .collect::<Option<Vec<Value>>>()
    else {
        return Ok(None);
    };
    if let Some(stats) = stats {
        stats.record_expand_topology_counts(seed_count as u64, far_ids.len() as u64);
    }
    let read_started = std::time::Instant::now();
    let presence = reader.get_vertex_property_columns_with_handles_batch(
        space_name,
        dst_tag,
        &unique,
        &[],
        &handles,
    )?;
    let read_us = read_started.elapsed().as_micros() as u64;
    let present: Vec<bool> = presence.iter().map(Option::is_some).collect();
    let missing = present.iter().filter(|found| !**found).count();

    let view_started = std::time::Instant::now();
    let mut out_keys = Vec::with_capacity(unique.len());
    let mut non_uniform_weight = false;
    for (index, found) in present.iter().enumerate() {
        if *found {
            out_keys.push(keys[index].clone());
            non_uniform_weight |= weights[index] != 1;
        }
    }
    let out_weights = if non_uniform_weight {
        present
            .iter()
            .enumerate()
            .filter_map(|(index, found)| found.then_some(weights[index]))
            .collect()
    } else {
        Vec::new()
    };
    let view_us = view_started.elapsed().as_micros() as u64;
    if let Some(stats) = stats {
        stats.record_expand_read_us(read_us);
        stats.record_expand_phases(0, read_us, 0, view_us, 0);
        stats.record_expand_destination_counts(unique.len() as u64, missing as u64);
        stats.record_expand_output_rows(out_keys.len() as u64);
    }
    if out_keys.is_empty() {
        return Ok(Some(ColumnarOutcome::Empty));
    }
    let payload = PackedExpandPayload {
        key_name: key_name.to_string(),
        keys: out_keys,
        weights: out_weights,
    };
    Ok(Some(ColumnarOutcome::Produced(
        DataChunk::new_packed_expand(output_layout, payload),
    )))
}

/// Single-step columnar assembly for closed-loop materialized hops.
///
/// Uses a projected edge read (topology only when no edge properties are
/// demanded), filters dangling destinations with one batched vertex read per
/// tag, and either skips the row view (rowless typed output) or keeps rows
/// with identity shortcuts for undemanded slots. Demanded properties ride
/// flat bypass columns (`{var}.{prop}`) in layout order in both modes.
/// Declines shapes the columnar path does not serve so callers fall back to
/// the row path instead of dropping input.
pub(super) fn expand_single_step_columnar(
    chunk: &DataChunk,
    output_layout: Arc<SlotLayout>,
    reader: &dyn QueryStorage,
    src_vids: Vec<Value>,
    ctx: &mut ExpandCtx,
) -> Result<ColumnarOutcome, QueryError> {
    if !ctx.closed_loop {
        return Ok(ColumnarOutcome::Declined);
    }
    let space_name = ctx.space_name;
    let edge_types = ctx.edge_types;
    let direction = ctx.direction;
    let seed_slot = seed_slot(&chunk.get_layout(), &ctx.col_names_template);
    let seed_width = if chunk.rows.is_empty() {
        chunk
            .typed_len()
            .map(|_| chunk.get_layout().len())
            .unwrap_or(0)
    } else {
        chunk
            .rows
            .first()
            .map_or(chunk.get_layout().len(), |r| r.len())
    };
    let input_layout = chunk.get_layout();
    let (edge_bypass_opt, dst_bypass_opt) = hop_bypass(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    if ctx.skip_rows && (edge_bypass_opt.is_none() || dst_bypass_opt.is_none()) {
        return Err(QueryError::execution(
            "rowless expand without complete bypass columns violates the plan contract".to_string(),
        ));
    }
    let conflict = has_bypass_conflict(
        &ctx.col_names_template,
        ctx.edge_required_props.as_ref(),
        ctx.dst_required_props.as_ref(),
        &input_layout,
    );
    let rowless =
        ctx.skip_rows && edge_bypass_opt.is_some() && dst_bypass_opt.is_some() && !conflict;
    let edge_bypass = edge_bypass_opt.clone().unwrap_or_default();
    let dst_bypass = dst_bypass_opt.clone().unwrap_or_default();
    let input_typed = chunk.typed_columns.clone();
    let (seed_vids, seed_rows, seed_positions) = parse_seeds(chunk, seed_slot, &src_vids);
    if seed_vids.is_empty() {
        return Ok(ColumnarOutcome::Empty);
    }
    let stats: Option<std::sync::Arc<ColumnarStats>> =
        ctx.runtime.as_ref().map(|runtime| runtime.columnar_stats());
    let projection = edge_projection(ctx.edge_required_props.as_deref());
    let projection_ref = projection.as_deref();
    let edge_empty = matches!(ctx.edge_required_props.as_ref(), Some(v) if v.is_empty());
    let seed_handles = if edge_empty && ctx.edge_required_props.is_some() {
        identity_seed_handles(chunk, seed_slot, &seed_positions)
    } else {
        Vec::new()
    };
    let dst_empty = matches!(ctx.dst_required_props.as_ref(), Some(v) if v.is_empty());

    struct PendingEdge {
        seed_idx: usize,
        topology: EdgeTopology,
        edge: Option<Edge>,
    }
    let mut pending: Vec<PendingEdge> = Vec::new();
    let mut parent_of_child: Vec<usize> = Vec::new();
    let mut all_dst: Vec<VertexId> = Vec::new();
    let mut packed_edges: Option<PackedEdgeTopology> = None;
    if edge_empty && ctx.edge_required_props.is_some() && edge_bypass.is_empty() {
        let topology_schema = ctx.topology_schema(reader)?;
        let read_started = std::time::Instant::now();
        let read = reader.get_packed_node_edge_headers_batch(
            space_name,
            &seed_vids,
            Some(&seed_handles),
            direction,
            edge_types,
            topology_schema.as_ref(),
        );
        let read_us = read_started.elapsed().as_micros() as u64;
        if let Some(stats) = &stats {
            stats.record_expand_read_us(read_us);
            stats.record_expand_phases(read_us, 0, 0, 0, 0);
        }
        let packed = read?;
        let identity_key = ctx
            .packed_key_name
            .clone()
            .zip(ctx.packed_identity_type.clone());
        if let Some((key_name, identity_type)) = identity_key {
            let key_property = key_name.rsplit('.').next().unwrap_or(&key_name);
            if edge_bypass.is_empty() && dst_bypass.len() == 1 && dst_bypass[0] == key_property {
                if packed.dst.is_empty() && ctx.include_empty_paths {
                    return Ok(ColumnarOutcome::Declined);
                }
                let planned_tag = ctx.dst_tag.to_string();
                let runtime_tag = ctx.resolved_dst_tag(reader);
                if runtime_tag.as_deref() == Some(planned_tag.as_str()) && !planned_tag.is_empty() {
                    if let Some(outcome) = expand_packed_identity(
                        reader,
                        space_name,
                        &planned_tag,
                        &packed,
                        direction,
                        &key_name,
                        &identity_type,
                        seed_vids.len(),
                        stats.as_deref(),
                        Arc::clone(&output_layout),
                    )? {
                        return Ok(outcome);
                    }
                }
            }
        }
        for parent in 0..packed.parent_count() {
            let parent_vid = packed.parents[parent];
            for index in packed.child_range(parent) {
                let far = match direction {
                    linkrs_core::EdgeDirection::Out => packed.dst[index],
                    linkrs_core::EdgeDirection::In => packed.src[index],
                    linkrs_core::EdgeDirection::Both => {
                        if packed.src[index] == parent_vid {
                            packed.dst[index]
                        } else {
                            packed.src[index]
                        }
                    }
                };
                parent_of_child.push(parent);
                all_dst.push(far);
            }
        }
        packed_edges = Some(packed);
    } else {
        let read_started = std::time::Instant::now();
        let read = reader.get_node_edges_projected_batch(
            space_name,
            &seed_vids,
            direction,
            edge_types,
            projection_ref,
        );
        let read_us = read_started.elapsed().as_micros() as u64;
        if let Some(stats) = &stats {
            stats.record_expand_read_us(read_us);
            stats.record_expand_phases(read_us, 0, 0, 0, 0);
        }
        let edge_batches = read?;
        for (seed_idx, (vid, edges)) in seed_vids.iter().zip(edge_batches.iter()).enumerate() {
            for edge in edges {
                let dst = edge_neighbor(edge, vid, direction);
                pending.push(PendingEdge {
                    seed_idx,
                    topology: EdgeTopology {
                        src: *edge.src(),
                        dst: *edge.dst(),
                        edge_type: Arc::from(edge.edge_type()),
                        ranking: edge.ranking(),
                    },
                    edge: Some(edge.clone()),
                });
                parent_of_child.push(seed_idx);
                all_dst.push(dst);
            }
        }
    }
    if let Some(stats) = &stats {
        stats.record_expand_topology_counts(seed_vids.len() as u64, all_dst.len() as u64);
    }
    if all_dst.is_empty() {
        // OPTIONAL MATCH must preserve seeds with no reachable neighbour.
        // The columnar path has no pending hops to project; decline so the
        // row path emits the NULL-filled rows instead of dropping them.
        if ctx.include_empty_paths {
            return Ok(ColumnarOutcome::Declined);
        }
        return Ok(ColumnarOutcome::Empty);
    }
    let tag_started = std::time::Instant::now();
    let dst_tag_owned = ctx
        .resolved_dst_tag(reader)
        .unwrap_or_else(|| ctx.dst_tag.to_string());
    let tag_us = tag_started.elapsed().as_micros() as u64;
    if let Some(stats) = &stats {
        stats.record_expand_phases(0, 0, tag_us, 0, 0);
    }
    let dst_tag = dst_tag_owned.as_str();
    let dest_started = std::time::Instant::now();
    let read = read_destinations(
        reader,
        space_name,
        dst_tag,
        &all_dst,
        dst_bypass_opt.as_ref(),
        rowless || dst_empty,
        packed_edges
            .as_ref()
            .map(|packed| packed.destination_handles.as_slice()),
    );
    let dest_us = dest_started.elapsed().as_micros() as u64;
    let read = read?;
    if let Some(stats) = &stats {
        stats.record_expand_read_us(dest_us);
        stats.record_expand_phases(0, read.read_us, 0, read.view_us, 0);
        stats.record_expand_destination_counts(read.unique as u64, read.missing as u64);
    }
    let destinations = read.view;
    let view_started = std::time::Instant::now();
    let mut kept: Vec<usize> = Vec::with_capacity(all_dst.len());
    for i in 0..destinations.len() {
        if destinations.present(i) {
            kept.push(i);
        }
    }
    let view_us = view_started.elapsed().as_micros() as u64;
    if let Some(stats) = &stats {
        stats.record_expand_phases(0, 0, 0, view_us, 0);
    }
    if kept.is_empty() {
        if let Some(stats) = &stats {
            stats.record_expand_output_rows(0);
        }
        return Ok(ColumnarOutcome::Empty);
    }
    if let Some(key_name) = ctx.packed_key_name.as_deref() {
        let key_property = key_name.rsplit('.').next().unwrap_or(key_name);
        let (direct, eligible) =
            if edge_bypass.is_empty() && dst_bypass.len() == 1 && dst_bypass[0] == key_property {
                (Some(dst_bypass[0].as_str()), true)
            } else if edge_bypass.is_empty() && dst_empty {
                (None, true)
            } else {
                // The plan selected a direct handoff but this chunk does not
                // carry every demanded column; keep the ordinary typed output path.
                (None, false)
            };
        if rowless && eligible {
            let keys: Vec<Value> = kept
                .iter()
                .map(|&index| match direct {
                    Some(property) => destinations.property(index, property),
                    None => destinations
                        .vertex(index)
                        .map(|vertex| Value::VertexId(*vertex.vid()))
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null)),
                })
                .collect();
            let payload = PackedExpandPayload {
                key_name: key_name.to_string(),
                weights: Vec::new(),
                keys,
            };
            if let Some(stats) = &stats {
                stats.record_expand_output_rows(payload.len() as u64);
            }
            return Ok(ColumnarOutcome::Produced(DataChunk::new_packed_expand(
                output_layout,
                payload,
            )));
        }
    }
    let output_started = std::time::Instant::now();

    if rowless {
        let mut edge_src = Vec::with_capacity(kept.len());
        let mut edge_dst = Vec::with_capacity(kept.len());
        let mut edge_types_out = Vec::with_capacity(kept.len());
        let mut rankings = Vec::with_capacity(kept.len());
        let mut dst_ids = Vec::with_capacity(kept.len());
        let mut gather_indices = Vec::with_capacity(kept.len());
        if let Some(ref packed) = packed_edges {
            for &i in &kept {
                edge_src.push(packed.src[i]);
                edge_dst.push(packed.dst[i]);
                edge_types_out.push(packed.edge_type(i));
                rankings.push(packed.rankings[i]);
                dst_ids.push(packed.dst[i]);
                gather_indices.push(seed_positions[parent_of_child[i]]);
            }
        } else {
            for &i in &kept {
                let req = &pending[i];
                edge_src.push(req.topology.src);
                edge_dst.push(req.topology.dst);
                edge_types_out.push(Arc::clone(&req.topology.edge_type));
                rankings.push(req.topology.ranking);
                dst_ids.push(req.topology.dst);
                gather_indices.push(seed_positions[req.seed_idx]);
            }
        }
        let mut typed: Vec<TypedColumn> =
            Vec::with_capacity(seed_width + 2 + edge_bypass.len() + dst_bypass.len());
        for slot in 0..seed_width {
            if let Some(ref cols) = input_typed {
                if let Some(col) = cols.get(slot) {
                    typed.push(gather_typed_column(col, &gather_indices));
                    continue;
                }
            }
            typed.push(TypedColumn::Fallback(
                kept.iter()
                    .map(|&i| {
                        seed_rows[parent_of_child[i]]
                            .get(slot)
                            .cloned()
                            .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                    })
                    .collect(),
            ));
        }
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_arc_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
        typed.push(TypedColumn::VertexIdentity(dst_ids));
        for prop in &edge_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| {
                        edge_bypass_value(
                            pending[i]
                                .edge
                                .as_ref()
                                .expect("edge property demand keeps the full edge"),
                            prop,
                        )
                    })
                    .collect(),
            ));
        }
        for prop in &dst_bypass {
            typed.push(bypass_column(
                kept.iter()
                    .map(|&i| destinations.property(i, prop))
                    .collect(),
            ));
        }
        check_rowless_lockstep(&output_layout, &typed, kept.len())?;
        let mut out = DataChunk::new_with_layout(Vec::new(), output_layout);
        out.typed_columns = Some(typed);
        if let Some(stats) = &stats {
            stats.record_expand_phases(0, 0, 0, 0, output_started.elapsed().as_micros() as u64);
            stats.record_expand_output_rows(kept.len() as u64);
        }
        return Ok(ColumnarOutcome::Produced(out));
    }

    let mut out_rows: Vec<Vec<Value>> = Vec::with_capacity(kept.len());
    let mut edge_src = Vec::with_capacity(kept.len());
    let mut edge_dst = Vec::with_capacity(kept.len());
    let mut edge_types_out = Vec::with_capacity(kept.len());
    let mut rankings = Vec::with_capacity(kept.len());
    let mut dst_ids = Vec::with_capacity(kept.len());
    let mut gather_indices = Vec::with_capacity(kept.len());
    // Destination assembly reuses the batched existence read above: it was
    // issued with the schema-resolved tag and is aligned with `pending`, so
    // no per-row point lookup is needed for any demand shape.
    for &i in &kept {
        let seed_idx = parent_of_child[i];
        let seed_row = &seed_rows[seed_idx];
        let (src, far_dst, edge_type, rank) = match packed_edges.as_ref() {
            Some(packed) => (
                packed.src[i],
                packed.dst[i],
                packed.edge_type(i),
                packed.rankings[i],
            ),
            None => {
                let req = &pending[i];
                (
                    req.topology.src,
                    req.topology.dst,
                    Arc::clone(&req.topology.edge_type),
                    req.topology.ranking,
                )
            }
        };
        let edge_value = if edge_empty {
            Value::edge_header(EdgeHeader::new(src, far_dst, edge_type.to_string(), rank))
        } else {
            Value::Edge(Box::new(
                pending[i]
                    .edge
                    .clone()
                    .expect("non-topology edge demand keeps the full edge"),
            ))
        };
        let dst_value = if dst_empty {
            Value::VertexId(far_dst)
        } else {
            match destinations.vertex(i) {
                Some(v) => Value::Vertex(Box::new(v.clone())),
                None => continue,
            }
        };
        let mut row = Vec::with_capacity(seed_row.len() + 2 + edge_bypass.len() + dst_bypass.len());
        row.extend_from_slice(seed_row);
        row.push(edge_value);
        row.push(dst_value);
        for prop in &edge_bypass {
            row.push(edge_bypass_value(
                pending[i]
                    .edge
                    .as_ref()
                    .expect("edge property demand keeps the full edge"),
                prop,
            ));
        }
        for prop in &dst_bypass {
            row.push(destinations.property(i, prop));
        }
        out_rows.push(row);
        edge_src.push(src);
        edge_dst.push(far_dst);
        edge_types_out.push(edge_type);
        rankings.push(rank);
        dst_ids.push(far_dst);
        gather_indices.push(seed_positions[seed_idx]);
    }
    if out_rows.is_empty() {
        if let Some(stats) = &stats {
            stats.record_expand_phases(0, 0, 0, 0, output_started.elapsed().as_micros() as u64);
            stats.record_expand_output_rows(0);
        }
        return Ok(ColumnarOutcome::Empty);
    }
    check_output_layout(
        &output_layout,
        seed_width + 2 + edge_bypass.len() + dst_bypass.len(),
        out_rows.first().map(Vec::len),
    )?;
    let mut out = DataChunk::new_with_layout(out_rows, Arc::clone(&output_layout));
    // Seed arms without an input typed column mirror the row view exactly, so
    // row-based and typed consumers observe the same seed values.
    let mut typed: Vec<TypedColumn> =
        Vec::with_capacity(seed_width + 2 + edge_bypass.len() + dst_bypass.len());
    for slot in 0..seed_width {
        if let Some(ref cols) = input_typed {
            if let Some(col) = cols.get(slot) {
                typed.push(gather_typed_column(col, &gather_indices));
                continue;
            }
        }
        typed.push(TypedColumn::Fallback(
            out.rows
                .iter()
                .map(|r| {
                    r.get(slot)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    if edge_empty {
        typed.push(TypedColumn::EdgeHeader(EdgeHeaderColumn::from_arc_parts(
            edge_src,
            edge_dst,
            edge_types_out,
            rankings,
        )));
    } else {
        typed.push(TypedColumn::Fallback(
            out.rows.iter().map(|r| r[seed_width].clone()).collect(),
        ));
    }
    if dst_empty {
        typed.push(TypedColumn::VertexIdentity(dst_ids));
    } else {
        typed.push(TypedColumn::Fallback(
            out.rows.iter().map(|r| r[seed_width + 1].clone()).collect(),
        ));
    }
    let bypass_base = seed_width + 2;
    for (j, _) in edge_bypass.iter().enumerate() {
        typed.push(bypass_column(
            out.rows
                .iter()
                .map(|r| {
                    r.get(bypass_base + j)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    for (j, _) in dst_bypass.iter().enumerate() {
        typed.push(bypass_column(
            out.rows
                .iter()
                .map(|r| {
                    r.get(bypass_base + edge_bypass.len() + j)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::NullType::Null))
                })
                .collect(),
        ));
    }
    check_row_lockstep(&output_layout, &typed, &out.rows)?;
    out.typed_columns = Some(typed);
    if let Some(stats) = &stats {
        stats.record_expand_phases(0, 0, 0, 0, output_started.elapsed().as_micros() as u64);
        stats.record_expand_output_rows(out.rows.len() as u64);
    }
    Ok(ColumnarOutcome::Produced(out))
}

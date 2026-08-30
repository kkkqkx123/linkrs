use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::cold::{ColdIndexEntry, ColdSnapshot};
use crate::edge::{EdgeRecord, EdgeStore, Nbr};
use crate::engine::data_store::EdgeTableKey;
use crate::engine::graph_storage::context::helpers;
use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::{
    edge_record_to_edge, edge_record_to_edge_projected, endpoint_label_id, serialize_properties,
    value_to_string, vertex_record_to_vertex,
};
use crate::engine::params::EdgeOperationParams;
use graphdb_core::types::{EdgeId, EdgeTypeInfo, LabelId, TagInfo, Timestamp, VertexId};
use graphdb_core::vertex_edge_path::Tag;
use graphdb_core::{Edge, EdgeDirection, StorageError, StorageResult, Value, Vertex};

use crate::engine::graph_storage::reader::cold::*;
use crate::engine::graph_storage::reader::utils::*;

/// Enable the per-table edge property index for `edge_type`.
pub(crate) fn enable_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    pool_capacity: u64,
) -> StorageResult<bool> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        ctx.enable_edge_property_index(src_label, dst_label, edge_label, pool_capacity)?;
    } else {
        // Unconstrained endpoint tags: enable on every table of this edge type.
        ctx.data_store()
            .with_edge_tables(|tables| -> StorageResult<()> {
                let matching: Vec<_> = tables
                    .values()
                    .filter(|arc| arc.read().0.label() == edge_label)
                    .cloned()
                    .collect();
                for arc in matching {
                    arc.write().0.enable_property_index(pool_capacity)?;
                }
                Ok(())
            })?;
    }
    Ok(true)
}

/// Whether the per-table edge property index is enabled for `edge_type`.
pub(crate) fn has_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<bool> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        Ok(ctx.has_edge_property_index(src_label, dst_label, edge_label))
    } else {
        Ok(ctx.data_store().with_edge_tables(|tables| {
            tables
                .values()
                .filter(|arc| arc.read().0.label() == edge_label)
                .any(|arc| arc.read().0.has_property_index())
        }))
    }
}

/// Drop the per-table edge property index for `edge_type`.
pub(crate) fn disable_edge_property_index(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
) -> StorageResult<()> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    if src_label != 0 && dst_label != 0 {
        ctx.disable_edge_property_index(src_label, dst_label, edge_label)?;
    } else {
        ctx.data_store()
            .with_edge_tables(|tables| -> StorageResult<()> {
                for arc in tables
                    .values()
                    .filter(|arc| arc.read().0.label() == edge_label)
                {
                    arc.write().0.disable_property_index();
                }
                Ok(())
            })?;
    }
    Ok(())
}

/// Look up edges of `edge_type` whose `prop_name` value falls in `[lower, upper)`.
///
/// Bounds are encoded with the ordered codec; the inclusion flags control
/// whether the boundary values themselves are part of the range.
#[allow(clippy::too_many_arguments)]
pub(crate) fn lookup_edges_by_property_range(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    prop_name: &str,
    lower: Option<&Value>,
    upper: Option<&Value>,
    include_lower: bool,
    include_upper: bool,
) -> StorageResult<Vec<Edge>> {
    record_schema_read(ctx, space);
    let (src_label, dst_label, edge_label) = resolve_edge_table_labels(ctx, space, edge_type)?;
    let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
    // Degenerate range [v, v) with an exclusive upper bound is interpreted as
    // a prefix/equality bound: everything from v up to the next value boundary.
    let prefix_bounds = include_lower && !include_upper && lower.is_some() && upper == lower;
    let value_lower = match lower {
        Some(value) => {
            let encoded = codec.encode(value)?;
            if include_lower {
                encoded
            } else {
                graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&encoded)
            }
        }
        None => Vec::new(),
    };
    let value_upper = match upper {
        Some(value) => {
            let encoded = codec.encode(value)?;
            if prefix_bounds || include_upper {
                graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&encoded)
            } else {
                encoded
            }
        }
        None => Vec::new(),
    };

    let ts = ctx.get_read_timestamp();
    let mut edges = Vec::new();

    let records = if src_label != 0 && dst_label != 0 {
        ctx.lookup_edges_by_property_range(
            src_label,
            dst_label,
            edge_label,
            prop_name,
            &value_lower,
            &value_upper,
            ts,
        )
    } else {
        ctx.data_store().with_edge_tables(|tables| {
            let matching: Vec<_> = tables
                .values()
                .filter(|arc| arc.read().0.label() == edge_label)
                .cloned()
                .collect();
            let mut records = Vec::new();
            for arc in matching {
                let table = arc.read();
                records.extend(
                    table
                        .0
                        .lookup_edges_by_property_range(prop_name, &value_lower, &value_upper)
                        .into_iter()
                        .filter_map(|(src, dst, rank)| table.0.get_edge(src, dst, rank, ts)),
                );
            }
            records
        })
    };

    for record in &records {
        let src_internal = record.src_vid.as_int64().unwrap_or(0) as u32;
        let dst_internal = record.dst_vid.as_int64().unwrap_or(0) as u32;
        let src_external = if src_label != 0 {
            ctx.get_external_id(src_label, src_internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(src_label, src_internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.src_vid))
        } else {
            ctx.get_external_id_any(src_internal, ts)
                .unwrap_or_else(|| format!("{}", record.src_vid))
        };
        let dst_external = if dst_label != 0 {
            ctx.get_external_id(dst_label, dst_internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(dst_label, dst_internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.dst_vid))
        } else {
            ctx.get_external_id_any(dst_internal, ts)
                .unwrap_or_else(|| format!("{}", record.dst_vid))
        };
        edges.push(edge_record_to_edge(
            record,
            edge_type,
            &src_external,
            &dst_external,
        ));
    }

    // Cold snapshot property index: same encoded bounds as the hot index.
    // Dedup happens in internal-ID space (the CSR row indices shared by the
    // hot lookup records and the cold index entries).
    let cold = ctx.cold_snapshots().read();
    if let Some(snapshots) = cold.get(&edge_label) {
        let mut seen: HashSet<(u32, u32, i64)> = records
            .iter()
            .map(|r| {
                (
                    r.src_vid.as_int64().unwrap_or(0) as u32,
                    r.dst_vid.as_int64().unwrap_or(0) as u32,
                    r.rank,
                )
            })
            .collect();
        for snapshot in snapshots.iter().filter(|s| ts >= s.snapshot_ts()) {
            let Some(index) = snapshot.property_index() else {
                continue;
            };
            if !index.has_property(prop_name) {
                continue;
            }
            for entry in index.lookup(prop_name, &value_lower, &value_upper) {
                let key = (entry.src_internal, entry.dst_internal, entry.rank);
                if seen.insert(key) {
                    edges.push(cold_index_entry_to_edge(
                        ctx, snapshot, &entry, edge_type, src_label, dst_label, ts,
                    ));
                }
            }
        }
    }

    Ok(edges)
}

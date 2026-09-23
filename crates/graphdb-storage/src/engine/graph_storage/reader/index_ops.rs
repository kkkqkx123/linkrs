use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::graph_storage::ops::edge_record_to_edge;
use graphdb_core::{Edge, StorageResult, Value};

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
                    .filter(|arc| arc.read().label() == edge_label)
                    .cloned()
                    .collect();
                for arc in matching {
                    arc.write().enable_property_index(pool_capacity)?;
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
                .filter(|arc| arc.read().label() == edge_label)
                .any(|arc| arc.read().has_property_index())
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
                    .filter(|arc| arc.read().label() == edge_label)
                {
                    arc.write().disable_property_index();
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
                .filter(|arc| arc.read().label() == edge_label)
                .cloned()
                .collect();
            let mut records = Vec::new();
            for arc in matching {
                let table = arc.read();
                records.extend(
                    table
                        .lookup_edges_by_property_range(prop_name, &value_lower, &value_upper)
                        .into_iter()
                        .filter_map(|(src, dst, rank)| table.get_edge(src, dst, rank, ts)),
                );
            }
            records
        })
    };

    for record in &records {
        let src_internal = record.src_vid.as_internal_u32();
        let dst_internal = record.dst_vid.as_internal_u32();
        let src_external = match src_internal {
            Some(internal) if src_label != 0 => ctx
                .get_external_id(src_label, internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(src_label, internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.src_vid)),
            Some(internal) => ctx
                .get_external_id_any(internal, ts)
                .unwrap_or_else(|| format!("{}", record.src_vid)),
            None => format!("{}", record.src_vid),
        };
        let dst_external = match dst_internal {
            Some(internal) if dst_label != 0 => ctx
                .get_external_id(dst_label, internal, ts)
                .or_else(|| {
                    ctx.get_external_id_by_internal_id(dst_label, internal)
                        .map(|v| vid_to_string(&v))
                })
                .unwrap_or_else(|| format!("{}", record.dst_vid)),
            Some(internal) => ctx
                .get_external_id_any(internal, ts)
                .unwrap_or_else(|| format!("{}", record.dst_vid)),
            None => format!("{}", record.dst_vid),
        };
        edges.push(edge_record_to_edge(
            record,
            edge_type,
            &src_external,
            &dst_external,
        ));
    }

    Ok(edges)
}

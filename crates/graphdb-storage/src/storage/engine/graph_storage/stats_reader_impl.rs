//! Engine-level column statistics snapshot resolution.
//!
//! Resolves `(space, tag/edge_type, column)` to physical storage and
//! aggregates optimizer-facing snapshots:
//! - vertex side: per-shard zone-map bounds merged across all shards of the
//!   tag's [`ShardedVertexTable`];
//! - edge side: zone-map aggregated stats merged across all partitions of
//!   the edge type.
//!
//! All reads take short-lived table locks and touch only chunk metadata —
//! no rows are materialized (the property-table path falls back to a column
//! scan only when no zone maps have been built yet).

use std::sync::Arc;

use super::context::GraphStorageContext;
use super::ops::{edge_label_id, tag_label_id};
use crate::storage::stats_reader::ColumnStatsSnapshot;

pub(crate) fn vertex_column_stats(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    column: &str,
) -> Option<Arc<ColumnStatsSnapshot>> {
    let label = tag_label_id(ctx, space, tag).ok()??;
    let snapshot = ctx.data_store().with_vertex_tables(|tables| {
        tables
            .get(&label)
            .and_then(|table| table.column_stats_snapshot(column))
    })?;
    Some(Arc::new(snapshot))
}

pub(crate) fn edge_column_stats(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    column: &str,
) -> Option<Arc<ColumnStatsSnapshot>> {
    let label = edge_label_id(ctx, space, edge_type).ok()??;
    let keys = ctx.data_store().edge_partition_keys(label).ok()?;
    if keys.is_empty() {
        return None;
    }

    let mut acc = ColumnStatsSnapshot::default();
    for key in &keys {
        let snapshot = ctx
            .data_store()
            .with_single_edge_table(key, |store| Ok(store.column_stats_snapshot(column)))
            .ok()??;
        acc.absorb(&snapshot);
    }
    if !acc.has_envelope() && acc.row_count == 0 {
        return None;
    }

    Some(Arc::new(acc))
}

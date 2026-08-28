use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};
use crate::cold::ColdSnapshot;
use crate::edge::{EdgeRecord, Nbr};
use crate::engine::data_store::EdgeTableKey;
use crate::engine::{EdgeOperationParams, InsertEdgeParams};
use crate::vertex::ShardedVertexTable;

use super::super::ops::endpoint_label_id;
use super::helpers;
use super::GraphStorageContext;

struct EdgeLabelLookupCtx<'a> {
    vertex_tables: &'a HashMap<LabelId, Arc<ShardedVertexTable>>,
    src_id: &'a VertexId,
    src_label: LabelId,
    dst_id: &'a VertexId,
    dst_label: LabelId,
    edge_label: LabelId,
    ts: Timestamp,
}

impl GraphStorageContext {
    pub fn insert_edge(&self, params: InsertEdgeParams) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let (src_internal, dst_internal, actual_src_label, actual_dst_label) =
            self.persistent.data_store.with_vertex_tables(
                |vertex_tables| -> StorageResult<(u32, u32, LabelId, LabelId)> {
                    let src_internal = helpers::resolve_internal_id(
                        self,
                        vertex_tables,
                        params.src_label,
                        params.src_id,
                        params.ts,
                    )
                    .ok_or(StorageError::vertex_not_found())?;
                    let dst_internal = helpers::resolve_internal_id(
                        self,
                        vertex_tables,
                        params.dst_label,
                        params.dst_id,
                        params.ts,
                    )
                    .ok_or(StorageError::vertex_not_found())?;
                    let actual_src_label = if params.src_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &params.src_id, params.ts)
                            .ok_or(StorageError::vertex_not_found())?
                    } else {
                        params.src_label
                    };
                    let actual_dst_label = if params.dst_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &params.dst_id, params.ts)
                            .ok_or(StorageError::vertex_not_found())?
                    } else {
                        params.dst_label
                    };
                    Ok((
                        src_internal,
                        dst_internal,
                        actual_src_label,
                        actual_dst_label,
                    ))
                },
            )?;

        let key = EdgeTableKey::new(actual_src_label, actual_dst_label, params.edge_label);
        let template_key = EdgeTableKey::new(0, 0, params.edge_label);

        // Lazily register snapshot for this edge partition if needed
        self.ensure_edge_snapshot_registered(key);
        let stats_manager = self.persistent.stats_manager.clone();
        let freeze_requested = self.persistent.data_store.with_edge_partition_mut(
            key,
            template_key,
            |template| {
                let mut s = template.schema().clone();
                s.src_label = actual_src_label;
                s.dst_label = actual_dst_label;
                let mut table = crate::edge::EdgeStore::new(s)?;
                if let Some(stats) = stats_manager {
                    table.set_stats_manager(stats);
                }
                Ok(table)
            },
            |edge_table| {
                edge_table.insert_edge(
                    src_internal,
                    dst_internal,
                    params.rank,
                    params.properties,
                    params.ts,
                )?;
                Ok(edge_table.needs_background_freeze())
            },
        )?;
        if freeze_requested {
            self.schedule_background_maintenance();
        }
        self.mark_edge_modified(params.edge_label);
        Ok(())
    }

    fn resolve_edge_table_key(ctx: EdgeLabelLookupCtx) -> EdgeTableKey {
        let actual_src_label = if ctx.src_label == 0 {
            helpers::resolve_internal_id_label(ctx.vertex_tables, ctx.src_id, ctx.ts)
                .unwrap_or(ctx.src_label)
        } else {
            ctx.src_label
        };
        let actual_dst_label = if ctx.dst_label == 0 {
            helpers::resolve_internal_id_label(ctx.vertex_tables, ctx.dst_id, ctx.ts)
                .unwrap_or(ctx.dst_label)
        } else {
            ctx.dst_label
        };
        EdgeTableKey::new(actual_src_label, actual_dst_label, ctx.edge_label)
    }

    /// Export a cold snapshot (`.lkcs` file) for one edge type at timestamp `ts`.
    ///
    /// The snapshot is written to `path` and also returned in memory so it can
    /// be registered immediately via [`Self::load_cold_snapshot`].
    pub fn export_cold_snapshot<P: AsRef<std::path::Path>>(
        &self,
        space: &str,
        edge_type: &str,
        ts: Timestamp,
        path: P,
    ) -> StorageResult<ColdSnapshot> {
        let edge_info = self
            .schema_manager()
            .get_edge_type(space, edge_type)?
            .ok_or_else(|| {
                StorageError::not_found(format!(
                    "Edge type {} not found in space {}",
                    edge_type, space
                ))
            })?;
        let src_label =
            endpoint_label_id(self, space, &edge_info.src_tag_name)?.ok_or_else(|| {
                StorageError::not_found(format!("No source tag for edge {}", edge_type))
            })?;
        let dst_label =
            endpoint_label_id(self, space, &edge_info.dst_tag_name)?.ok_or_else(|| {
                StorageError::not_found(format!("No destination tag for edge {}", edge_type))
            })?;
        let key = EdgeTableKey::new(src_label, dst_label, edge_info.edge_type_id);
        self.persistent
            .data_store
            .with_single_edge_table(&key, |table| table.export_snapshot_file(ts, path))
    }

    pub fn get_edge(&self, params: &EdgeOperationParams, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let (src_internal, dst_internal, key) = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> Option<(u32, u32, EdgeTableKey)> {
                let src_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.src_label,
                    params.src_id,
                    ts,
                )?;
                let dst_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.dst_label,
                    params.dst_id,
                    ts,
                )?;
                let key = Self::resolve_edge_table_key(EdgeLabelLookupCtx {
                    vertex_tables,
                    src_id: &params.src_id,
                    src_label: params.src_label,
                    dst_id: &params.dst_id,
                    dst_label: params.dst_label,
                    edge_label: params.edge_label,
                    ts,
                });
                Some((src_internal, dst_internal, key))
            },
        )?;

        // Lazily register snapshot for this edge partition if needed
        self.ensure_edge_snapshot_registered(key);

        self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables.get(&key).and_then(|arc| {
                arc.read()
                    .get_edge(src_internal, dst_internal, params.rank, ts)
            })
        })
    }

    pub fn delete_edge(&self, params: &EdgeOperationParams, ts: Timestamp) -> StorageResult<bool> {
        self.delete_edge_impl(params, None, None, ts)
    }

    pub fn delete_edge_by_offset(
        &self,
        params: &EdgeOperationParams,
        oe_offset: i32,
        ie_offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        self.delete_edge_impl(params, Some(oe_offset), Some(ie_offset), ts)
    }

    fn delete_edge_impl(
        &self,
        params: &EdgeOperationParams,
        oe_offset: Option<i32>,
        ie_offset: Option<i32>,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let Some((src_internal, dst_internal, key)) = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let src_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.src_label,
                    params.src_id,
                    ts,
                )
                .or_else(|| {
                    helpers::resolve_internal_id_any(vertex_tables, params.src_label, params.src_id)
                })?;
                let dst_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.dst_label,
                    params.dst_id,
                    ts,
                )
                .or_else(|| {
                    helpers::resolve_internal_id_any(vertex_tables, params.dst_label, params.dst_id)
                })?;
                let key = Self::resolve_edge_table_key(EdgeLabelLookupCtx {
                    vertex_tables,
                    src_id: &params.src_id,
                    src_label: params.src_label,
                    dst_id: &params.dst_id,
                    dst_label: params.dst_label,
                    edge_label: params.edge_label,
                    ts,
                });
                Some((src_internal, dst_internal, key))
            })
        else {
            // Deleting an edge whose endpoints do not exist is a no-op.
            return Ok(false);
        };

        // Lazily register snapshot for this edge partition if needed
        self.ensure_edge_snapshot_registered(key);

        let deleted =
            self.persistent
                .data_store
                .with_single_edge_table_mut(&key, |edge_table| match (oe_offset, ie_offset) {
                    (Some(oe), Some(ie)) => edge_table.delete_edge_by_offset(
                        src_internal,
                        dst_internal,
                        params.rank,
                        oe,
                        ie,
                        ts,
                    ),
                    _ => edge_table.delete_edge(src_internal, dst_internal, params.rank, ts),
                })?;
        if deleted {
            self.mark_edge_modified(params.edge_label);
        }

        Ok(deleted)
    }

    pub fn out_edges(
        &self,
        edge_label: LabelId,
        src_label: LabelId,
        _dst_label: LabelId,
        src_id: VertexId,
        ts: Timestamp,
    ) -> Option<Vec<EdgeRecord>> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let (src_internal, actual_src) =
            self.persistent
                .data_store
                .with_vertex_tables(|vertex_tables| {
                    let src_internal =
                        helpers::resolve_internal_id(self, vertex_tables, src_label, src_id, ts)?;
                    let actual_src = if src_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &src_id, ts)
                            .unwrap_or(src_label)
                    } else {
                        src_label
                    };
                    Some((src_internal, actual_src))
                })?;

        // Lazily register snapshots for every matching edge partition.
        for key in self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables
                .keys()
                .copied()
                .filter(|key| key.edge_label == edge_label && key.src_label == actual_src)
                .collect::<Vec<_>>()
        }) {
            self.ensure_edge_snapshot_registered(key);
        }

        let records = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut records = Vec::new();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                records.extend(table.out_edges(src_internal, ts));
            }
            records
        });
        Some(records)
    }

    /// Raw out-edge neighbors of `src` (no `EdgeRecord` materialization, no
    /// property decode).  The neighbor endpoint is encoded in `Nbr.neighbor`.
    /// Returns the resolved internal src id together with the neighbors.
    pub fn out_nbrs(
        &self,
        edge_label: LabelId,
        src_label: LabelId,
        _dst_label: LabelId,
        src_id: VertexId,
        ts: Timestamp,
    ) -> Option<(u32, Vec<Nbr>)> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let (src_internal, actual_src) =
            self.persistent
                .data_store
                .with_vertex_tables(|vertex_tables| {
                    let src_internal =
                        helpers::resolve_internal_id(self, vertex_tables, src_label, src_id, ts)?;
                    let actual_src = if src_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &src_id, ts)
                            .unwrap_or(src_label)
                    } else {
                        src_label
                    };
                    Some((src_internal, actual_src))
                })?;

        // Lazily register snapshots for every matching edge partition.
        for key in self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables
                .keys()
                .copied()
                .filter(|key| key.edge_label == edge_label && key.src_label == actual_src)
                .collect::<Vec<_>>()
        }) {
            self.ensure_edge_snapshot_registered(key);
        }

        let nbrs = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut nbrs = Vec::new();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                nbrs.extend(table.merged_out_nbrs(src_internal, ts));
            }
            nbrs
        });
        Some((src_internal, nbrs))
    }

    pub fn in_edges(
        &self,
        edge_label: LabelId,
        _src_label: LabelId,
        dst_label: LabelId,
        dst_id: VertexId,
        ts: Timestamp,
    ) -> Option<Vec<EdgeRecord>> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let (dst_internal, actual_dst) =
            self.persistent
                .data_store
                .with_vertex_tables(|vertex_tables| {
                    let dst_internal =
                        helpers::resolve_internal_id(self, vertex_tables, dst_label, dst_id, ts)?;
                    let actual_dst = if dst_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &dst_id, ts)
                            .unwrap_or(dst_label)
                    } else {
                        dst_label
                    };
                    Some((dst_internal, actual_dst))
                })?;

        // Lazily register snapshots for every matching edge partition.
        for key in self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables
                .keys()
                .copied()
                .filter(|key| key.edge_label == edge_label && key.dst_label == actual_dst)
                .collect::<Vec<_>>()
        }) {
            self.ensure_edge_snapshot_registered(key);
        }

        let records = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut records = Vec::new();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                records.extend(table.in_edges(dst_internal, ts));
            }
            records
        });
        Some(records)
    }

    /// Raw in-edge neighbors of `dst` (no `EdgeRecord` materialization, no
    /// property decode).  The neighbor endpoint is encoded in `Nbr.neighbor`.
    /// Returns the resolved internal dst id together with the neighbors.
    pub fn in_nbrs(
        &self,
        edge_label: LabelId,
        _src_label: LabelId,
        dst_label: LabelId,
        dst_id: VertexId,
        ts: Timestamp,
    ) -> Option<(u32, Vec<Nbr>)> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return None;
        }

        let (dst_internal, actual_dst) =
            self.persistent
                .data_store
                .with_vertex_tables(|vertex_tables| {
                    let dst_internal =
                        helpers::resolve_internal_id(self, vertex_tables, dst_label, dst_id, ts)?;
                    let actual_dst = if dst_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &dst_id, ts)
                            .unwrap_or(dst_label)
                    } else {
                        dst_label
                    };
                    Some((dst_internal, actual_dst))
                })?;

        // Lazily register snapshots for every matching edge partition.
        for key in self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables
                .keys()
                .copied()
                .filter(|key| key.edge_label == edge_label && key.dst_label == actual_dst)
                .collect::<Vec<_>>()
        }) {
            self.ensure_edge_snapshot_registered(key);
        }

        let nbrs = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut nbrs = Vec::new();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                nbrs.extend(table.merged_in_nbrs(dst_internal, ts));
            }
            nbrs
        });
        Some((dst_internal, nbrs))
    }
}

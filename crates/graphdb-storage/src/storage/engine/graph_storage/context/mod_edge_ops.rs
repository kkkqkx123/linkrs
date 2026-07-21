use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::EdgeRecord;
use crate::storage::engine::data_store::EdgeTableKey;
use crate::storage::engine::{EdgeOperationParams, InsertEdgeParams};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::GraphStorageContext;
use super::helpers;

struct EdgeLabelLookupCtx<'a> {
    vertex_tables: &'a HashMap<LabelId, crate::storage::vertex::VertexTable>,
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
        let stats_manager = self.persistent.stats_manager.clone();
        self.persistent.data_store.with_edge_partition_mut(
            key,
            template_key,
            |template| {
                let mut s = template.schema().clone();
                s.src_label = actual_src_label;
                s.dst_label = actual_dst_label;
                let mut table = crate::storage::edge::EdgeStore::new(s)?;
                if let Some(stats) = stats_manager {
                    table.set_stats_manager(stats);
                }
                Ok(table)
            },
            |edge_table| {
                let mut rank = params.rank;
                loop {
                    match edge_table.insert_edge(
                        src_internal,
                        dst_internal,
                        rank,
                        params.properties,
                        params.ts,
                    ) {
                        Ok(()) => return Ok(()),
                        Err(ref error)
                            if error.kind()
                                == crate::core::error::storage::StorageErrorKind::EdgeAlreadyExists
                                && rank == params.rank =>
                        {
                            rank += 1;
                        }
                        Err(error) => return Err(error),
                    }
                }
            },
        )?;
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

        self.persistent.data_store.with_edge_tables(|edge_tables| {
            edge_tables.get(&key).and_then(|edge_table| {
                edge_table.get_edge(src_internal, dst_internal, params.rank, ts)
            })
        })
    }

    pub fn delete_edge(&self, params: &EdgeOperationParams, ts: Timestamp) -> StorageResult<bool> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }

        let (src_internal, dst_internal, key) = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> StorageResult<(u32, u32, EdgeTableKey)> {
                let src_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.src_label,
                    params.src_id,
                    ts,
                )
                .or_else(|| {
                    helpers::resolve_internal_id_any(vertex_tables, params.src_label, params.src_id)
                })
                .ok_or(StorageError::vertex_not_found())?;
                let dst_internal = helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.dst_label,
                    params.dst_id,
                    ts,
                )
                .or_else(|| {
                    helpers::resolve_internal_id_any(vertex_tables, params.dst_label, params.dst_id)
                })
                .ok_or(StorageError::vertex_not_found())?;
                let key = Self::resolve_edge_table_key(EdgeLabelLookupCtx {
                    vertex_tables,
                    src_id: &params.src_id,
                    src_label: params.src_label,
                    dst_id: &params.dst_id,
                    dst_label: params.dst_label,
                    edge_label: params.edge_label,
                    ts,
                });
                Ok((src_internal, dst_internal, key))
            },
        )?;

        let deleted = self
            .persistent
            .data_store
            .with_edge_tables_mut(|edge_tables| {
                let edge_table = edge_tables.get_mut(&key).ok_or_else(|| {
                    StorageError::label_not_found(format!("edge label {}", params.edge_label))
                })?;
                edge_table.delete_edge(src_internal, dst_internal, params.rank, ts)
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

        let records = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut records = Vec::new();
            for table in edge_tables
                .values()
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                records.extend(table.out_edges(src_internal, ts));
            }
            records
        });
        Some(records)
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

        let records = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut records = Vec::new();
            for table in edge_tables
                .values()
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                records.extend(table.in_edges(dst_internal, ts));
            }
            records
        });
        Some(records)
    }
}

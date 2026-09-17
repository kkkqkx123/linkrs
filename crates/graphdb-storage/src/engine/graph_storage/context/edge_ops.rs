use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::edge::{EdgeRecord, Nbr};
use crate::engine::data_store::EdgeTableKey;
use crate::engine::{EdgeOperationParams, InsertEdgeParams};
use crate::mvcc_visibility::PendingGate;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

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
        let stats_manager = self.persistent.stats_manager.clone();
        let maintenance_requested = self.persistent.data_store.with_edge_partition_mut(
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
                Ok(edge_table.needs_background_maintenance())
            },
        )?;
        if maintenance_requested {
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

        // Pending-aware recheck of the point lookup: the plain timestamp
        // predicate inside `get_edge` cannot see slot states, so a foreign
        // uncommitted creation would leak (dirty read) and a foreign
        // uncommitted deletion would hide a live edge. Recheck through the
        // gate; a creation owned by a foreign pending transaction hides the
        // edge, a foreign pending deletion is ignored by re-reading below it.
        // Scan-class funnels share the same gate through the table
        // `*_with_gate` methods.
        let own_write = self
            .operation_context
            .as_ref()
            .and_then(|context| context.write_timestamp);
        let gate = PendingGate::new(&self.persistent.version_manager, own_write);
        let mut cur = ts;
        loop {
            let record = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).and_then(|arc| {
                    arc.read()
                        .get_edge(src_internal, dst_internal, params.rank, cur)
                })
            });
            let edge_id = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).and_then(|arc| {
                    arc.read()
                        .edge_id_of(src_internal, dst_internal, params.rank, cur)
                })
            });
            let Some(edge_id) = edge_id else {
                return record;
            };
            let visible = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).map(|arc| {
                    arc.read()
                        .mvcc
                        .is_edge_visible_with_gate(edge_id, cur, &gate)
                })
            });
            if visible == Some(true) && record.is_some() {
                return record;
            }
            if record.is_some() {
                // Visible to the plain predicate but hidden through the
                // gate: the creation stamp belongs to a foreign uncommitted
                // transaction (dirty read filtered).
                return None;
            }
            // No record: a foreign pending deletion may be hiding a live edge.
            let delete_ts = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables
                    .get(&key)
                    .and_then(|arc| arc.read().mvcc.deletion_ts_of(edge_id))
            });
            if let Some(delete_ts) = delete_ts {
                if delete_ts <= cur && gate.is_foreign_pending(cur, delete_ts) && delete_ts > 0 {
                    cur = delete_ts - 1;
                    continue;
                }
            }
            return None;
        }
    }

    /// Projected point lookup: same pending-aware recheck as `get_edge`
    /// but decodes only `projection` (`None` = all columns, `Some(&[])` =
    /// topology only). Empty query projection means all columns; topology-only
    /// is requested explicitly with an empty slice via batch/cursor paths.
    pub fn get_edge_projected(
        &self,
        params: &EdgeOperationParams,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<EdgeRecord> {
        if !self
            .persistent
            .is_open
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        let (src_internal, dst_internal, key) = self.persistent.data_store.with_vertex_tables(
            |vertex_tables| -> Option<(u32, u32, EdgeTableKey)> {
                let src_internal = super::helpers::resolve_internal_id(
                    self,
                    vertex_tables,
                    params.src_label,
                    params.src_id,
                    ts,
                )?;
                let dst_internal = super::helpers::resolve_internal_id(
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
        let own_write = self
            .operation_context
            .as_ref()
            .and_then(|context| context.write_timestamp);
        let gate = PendingGate::new(&self.persistent.version_manager, own_write);
        let mut cur = ts;
        loop {
            let record = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).and_then(|arc| {
                    arc.read().get_edge_with_gate_projected(
                        src_internal,
                        dst_internal,
                        params.rank,
                        cur,
                        &gate,
                        projection,
                    )
                })
            });
            let edge_id = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).and_then(|arc| {
                    arc.read()
                        .edge_id_of(src_internal, dst_internal, params.rank, cur)
                })
            });
            let Some(edge_id) = edge_id else {
                return record;
            };
            let visible = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables.get(&key).map(|arc| {
                    arc.read()
                        .mvcc
                        .is_edge_visible_with_gate(edge_id, cur, &gate)
                })
            });
            if visible == Some(true) && record.is_some() {
                return record;
            }
            if record.is_some() {
                return None;
            }
            let delete_ts = self.persistent.data_store.with_edge_tables(|edge_tables| {
                edge_tables
                    .get(&key)
                    .and_then(|arc| arc.read().mvcc.deletion_ts_of(edge_id))
            });
            if let Some(delete_ts) = delete_ts {
                if delete_ts <= cur && gate.is_foreign_pending(cur, delete_ts) && delete_ts > 0 {
                    cur = delete_ts - 1;
                    continue;
                }
            }
            return None;
        }
    }

    pub fn delete_edge(&self, params: &EdgeOperationParams, ts: Timestamp) -> StorageResult<bool> {
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
            return Ok(false);
        };

        let deleted = self
            .persistent
            .data_store
            .with_single_edge_table_mut(&key, |edge_table| {
                edge_table.delete_edge(src_internal, dst_internal, params.rank, ts)
            })?;
        if deleted {
            self.mark_edge_modified(params.edge_label);
        }

        Ok(deleted)
    }

    /// Physically erase an edge created by an uncommitted insert.
    ///
    /// Insert-undo entry point (see `UndoTarget::delete_edge`): aborts must
    /// leave no trace in any of the three copies, unlike user deletes which
    /// go through logical deletion. Missing endpoints, partitions or edges
    /// report `Ok(false)` so undo replay stays idempotent.
    pub fn erase_inserted_edge(
        &self,
        params: &EdgeOperationParams,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        let resolved = self
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
            });
        let Some((src_internal, dst_internal, key)) = resolved else {
            return Ok(false);
        };
        if self
            .persistent
            .data_store
            .try_get_edge_table_mut(&key)
            .is_none()
        {
            return Ok(false);
        }
        let erased = self
            .persistent
            .data_store
            .with_single_edge_table_mut(&key, |edge_table| {
                Ok(edge_table.erase_edge(src_internal, dst_internal, params.rank, ts))
            })?;
        if erased {
            self.mark_edge_modified(params.edge_label);
        }
        Ok(erased)
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

        let nbrs = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut nbrs = Vec::new();
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                nbrs.extend(table.merged_out_nbrs_with_gate(src_internal, ts, &gate));
            }
            nbrs
        });
        Some((src_internal, nbrs))
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

        let nbrs = self.persistent.data_store.with_edge_tables(|edge_tables| {
            let mut nbrs = Vec::new();
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                nbrs.extend(table.merged_in_nbrs_with_gate(dst_internal, ts, &gate));
            }
            nbrs
        });
        Some((dst_internal, nbrs))
    }

    pub fn out_edges_projected(
        &self,
        edge_label: LabelId,
        src_label: LabelId,
        src_id: VertexId,
        ts: Timestamp,
        projection: Option<&[String]>,
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
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                records.extend(table.out_edges_with_gate_projected(
                    src_internal,
                    ts,
                    &gate,
                    projection,
                ));
            }
            records
        });
        Some(records)
    }

    pub fn in_edges_projected(
        &self,
        edge_label: LabelId,
        dst_label: LabelId,
        dst_id: VertexId,
        ts: Timestamp,
        projection: Option<&[String]>,
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
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                records.extend(table.in_edges_with_gate_projected(
                    dst_internal,
                    ts,
                    &gate,
                    projection,
                ));
            }
            records
        });
        Some(records)
    }

    pub fn out_edges_projected_limit(
        &self,
        edge_label: LabelId,
        src_label: LabelId,
        src_id: VertexId,
        ts: Timestamp,
        projection: Option<&[String]>,
        limit: usize,
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
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                let remaining = limit.saturating_sub(records.len());
                if remaining == 0 {
                    break;
                }
                records.extend(table.out_edges_with_gate_projected_limit(
                    src_internal,
                    ts,
                    &gate,
                    projection,
                    remaining,
                ));
            }
            records
        });
        Some(records)
    }

    pub fn in_edges_projected_limit(
        &self,
        edge_label: LabelId,
        dst_label: LabelId,
        dst_id: VertexId,
        ts: Timestamp,
        projection: Option<&[String]>,
        limit: usize,
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
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                let remaining = limit.saturating_sub(records.len());
                if remaining == 0 {
                    break;
                }
                records.extend(table.in_edges_with_gate_projected_limit(
                    dst_internal,
                    ts,
                    &gate,
                    projection,
                    remaining,
                ));
            }
            records
        });
        Some(records)
    }
}

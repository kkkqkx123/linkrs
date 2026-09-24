use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::edge::{BatchInsertEntry, EdgeRecord, HotNbr};
use crate::engine::data_store::EdgeTableKey;
use crate::engine::{
    DeleteEdgesBatchParams, EdgeOperationParams, InsertEdgeParams, InsertEdgesBatchParams,
};
use crate::mvcc_visibility::PendingGate;
use crate::vertex::ShardedVertexTable;
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

use super::helpers;
use super::GraphStorageContext;

type LabelPair = (LabelId, LabelId);
type EdgeEndpoints = Vec<(u32, u32, i64)>;
type PartitionedEdges = Vec<(LabelPair, EdgeEndpoints)>;

/// Project a stored internal endpoint id to its external `VertexId` using the
/// edge table's owning label. Unresolved ids stay unchanged (dangling edge).
fn endpoint_to_external(
    ctx: &GraphStorageContext,
    label: LabelId,
    vid: VertexId,
    ts: Timestamp,
) -> VertexId {
    if label == 0 {
        return vid;
    }
    match vid.as_internal_u32() {
        Some(internal) => ctx
            .get_external_id_by_internal_id(label, internal)
            .or_else(|| ctx.get_external_vertex_id(label, internal, ts))
            .unwrap_or(vid),
        None => vid,
    }
}

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

    /// Insert many edges of one edge type with one staging commit per owner
    /// partition.
    ///
    /// Every endpoint resolves once under a single vertex-table read, then
    /// edges group by owner partition so each edge lands exactly where
    /// repeated single inserts put it. One partition lock and one staging
    /// commit serve each group instead of one per edge.
    pub fn insert_edges_batch(&self, params: InsertEdgesBatchParams) -> StorageResult<()> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        if params.edges.is_empty() {
            return Ok(());
        }
        let resolved: Vec<(u32, u32, LabelId, LabelId)> =
            self.persistent.data_store.with_vertex_tables(
                |vertex_tables| -> StorageResult<Vec<(u32, u32, LabelId, LabelId)>> {
                    let mut resolved = Vec::with_capacity(params.edges.len());
                    for edge in params.edges {
                        let src_internal = helpers::resolve_internal_id(
                            self,
                            vertex_tables,
                            params.src_label,
                            edge.src_id,
                            params.ts,
                        )
                        .ok_or(StorageError::vertex_not_found())?;
                        let dst_internal = helpers::resolve_internal_id(
                            self,
                            vertex_tables,
                            params.dst_label,
                            edge.dst_id,
                            params.ts,
                        )
                        .ok_or(StorageError::vertex_not_found())?;
                        let actual_src_label = if params.src_label == 0 {
                            helpers::resolve_internal_id_label(
                                vertex_tables,
                                &edge.src_id,
                                params.ts,
                            )
                            .ok_or(StorageError::vertex_not_found())?
                        } else {
                            params.src_label
                        };
                        let actual_dst_label = if params.dst_label == 0 {
                            helpers::resolve_internal_id_label(
                                vertex_tables,
                                &edge.dst_id,
                                params.ts,
                            )
                            .ok_or(StorageError::vertex_not_found())?
                        } else {
                            params.dst_label
                        };
                        resolved.push((
                            src_internal,
                            dst_internal,
                            actual_src_label,
                            actual_dst_label,
                        ));
                    }
                    Ok(resolved)
                },
            )?;

        let mut by_partition: HashMap<(LabelId, LabelId), Vec<usize>> = HashMap::new();
        for (index, (_, _, actual_src, actual_dst)) in resolved.iter().enumerate() {
            by_partition
                .entry((*actual_src, *actual_dst))
                .or_default()
                .push(index);
        }
        let mut partitions: Vec<((LabelId, LabelId), Vec<usize>)> =
            by_partition.into_iter().collect();
        partitions.sort_unstable_by_key(|(key, _)| *key);

        let stats_manager = self.persistent.stats_manager.clone();
        let mut maintenance_requested = false;
        for ((actual_src, actual_dst), indices) in partitions {
            let key = EdgeTableKey::new(actual_src, actual_dst, params.edge_label);
            let template_key = EdgeTableKey::new(0, 0, params.edge_label);
            let entries: Vec<BatchInsertEntry> = indices
                .iter()
                .map(|&index| {
                    let (src_internal, dst_internal, _, _) = resolved[index];
                    let edge = &params.edges[index];
                    (
                        src_internal,
                        dst_internal,
                        edge.rank,
                        edge.properties,
                        params.ts,
                    )
                })
                .collect();
            let stats_manager = stats_manager.clone();
            let requested = self.persistent.data_store.with_edge_partition_mut(
                key,
                template_key,
                |template| {
                    let mut s = template.schema().clone();
                    s.src_label = actual_src;
                    s.dst_label = actual_dst;
                    let mut table = crate::edge::EdgeStore::new(s)?;
                    if let Some(stats) = stats_manager {
                        table.set_stats_manager(stats);
                    }
                    Ok(table)
                },
                |edge_table| {
                    edge_table.insert_edges_batch(&entries)?;
                    Ok(edge_table.needs_background_maintenance())
                },
            )?;
            maintenance_requested |= requested;
        }
        if maintenance_requested {
            self.schedule_background_maintenance();
        }
        self.mark_edge_modified(params.edge_label);
        Ok(())
    }

    /// Delete many edges of one edge type with one staging commit per owner
    /// partition.
    ///
    /// Every endpoint resolves once under a single vertex-table read, then
    /// keys group by owner partition so each delete lands exactly where
    /// repeated single deletes put it. One partition lock and one staging
    /// commit serve each group instead of one per edge. Keys whose endpoints
    /// do not resolve delete nothing and report no error, mirroring the
    /// single-delete no-op; keys resolving to tombstoned edges fail through
    /// the table batch like the single path. A missing partition table
    /// errors like the single path. Returns the number of net applied
    /// deletes.
    pub fn delete_edges_batch(&self, params: DeleteEdgesBatchParams) -> StorageResult<usize> {
        if !self.persistent.is_open.load(Ordering::Acquire) {
            return Err(StorageError::storage_not_open());
        }
        if params.edges.is_empty() {
            return Ok(0);
        }
        let resolved: Vec<Option<(u32, u32, LabelId, LabelId)>> = self
            .persistent
            .data_store
            .with_vertex_tables(|vertex_tables| {
                let mut resolved = Vec::with_capacity(params.edges.len());
                for edge in params.edges {
                    let src_internal = helpers::resolve_internal_id(
                        self,
                        vertex_tables,
                        params.src_label,
                        edge.src_id,
                        params.ts,
                    )
                    .or_else(|| {
                        helpers::resolve_internal_id_any(
                            vertex_tables,
                            params.src_label,
                            edge.src_id,
                        )
                    });
                    let dst_internal = helpers::resolve_internal_id(
                        self,
                        vertex_tables,
                        params.dst_label,
                        edge.dst_id,
                        params.ts,
                    )
                    .or_else(|| {
                        helpers::resolve_internal_id_any(
                            vertex_tables,
                            params.dst_label,
                            edge.dst_id,
                        )
                    });
                    let (Some(src_internal), Some(dst_internal)) = (src_internal, dst_internal)
                    else {
                        resolved.push(None);
                        continue;
                    };
                    let actual_src_label = if params.src_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &edge.src_id, params.ts)
                            .unwrap_or(params.src_label)
                    } else {
                        params.src_label
                    };
                    let actual_dst_label = if params.dst_label == 0 {
                        helpers::resolve_internal_id_label(vertex_tables, &edge.dst_id, params.ts)
                            .unwrap_or(params.dst_label)
                    } else {
                        params.dst_label
                    };
                    resolved.push(Some((
                        src_internal,
                        dst_internal,
                        actual_src_label,
                        actual_dst_label,
                    )));
                }
                resolved
            });

        let mut by_partition: HashMap<LabelPair, EdgeEndpoints> = HashMap::new();
        for (edge, slot) in params.edges.iter().zip(resolved.iter()) {
            if let Some((src_internal, dst_internal, actual_src, actual_dst)) = slot {
                by_partition
                    .entry((*actual_src, *actual_dst))
                    .or_default()
                    .push((*src_internal, *dst_internal, edge.rank));
            }
        }
        let mut partitions: PartitionedEdges = by_partition.into_iter().collect();
        partitions.sort_unstable_by_key(|(key, _)| *key);

        let mut applied = 0usize;
        let mut maintenance_requested = false;
        for ((actual_src, actual_dst), keys) in partitions {
            let key = EdgeTableKey::new(actual_src, actual_dst, params.edge_label);
            let (count, requested) =
                self.persistent
                    .data_store
                    .with_single_edge_table_mut(&key, |edge_table| {
                        let count = edge_table.delete_edges_batch(&keys, params.ts)?;
                        Ok((count, edge_table.needs_background_maintenance()))
                    })?;
            applied += count;
            maintenance_requested |= requested;
        }
        if maintenance_requested {
            self.schedule_background_maintenance();
        }
        self.mark_edge_modified(params.edge_label);
        Ok(applied)
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
        // Single table lock: the fused lookup resolves the record and its
        // edge id in one row scan, and the gate verdict reuses that id with
        // no second scan and no second lock acquisition. One snapshot closes
        // the lock-to-lock race window the old triple acquisition retried
        // against, so no retry loop remains: no match reads empty, and a
        // gate-hidden match is a filtered dirty read (a foreign uncommitted
        // creation leaking through the plain predicate).
        self.persistent.data_store.with_edge_tables(|edge_tables| {
            let guard = edge_tables.get(&key)?.read();
            let (record, edge_id) =
                guard.get_edge_with_id(src_internal, dst_internal, params.rank, ts)?;
            if guard.mvcc.is_edge_visible_with_gate(edge_id, ts, &gate) {
                Some(record)
            } else {
                None
            }
        })
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
        // Single table lock mirroring `get_edge`: the fused lookup resolves
        // the projected record and its edge id in one gate-aware row scan,
        // and the recheck below reuses that id with no second scan and no
        // second lock acquisition. One snapshot closes the lock-to-lock race
        // window, so the old retry loop collapses to a single pass.
        self.persistent.data_store.with_edge_tables(|edge_tables| {
            let guard = edge_tables.get(&key)?.read();
            let (record, edge_id) = guard.get_edge_projected_with_id(
                src_internal,
                dst_internal,
                params.rank,
                ts,
                &gate,
                projection,
            )?;
            if guard.mvcc.is_edge_visible_with_gate(edge_id, ts, &gate) {
                Some(record)
            } else {
                None
            }
        })
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

    /// Visit raw out-edge neighbors of `src` without building a vector.
    ///
    /// Zero-copy fan-out entry point: resolves the internal id once, then
    /// streams every visible neighbor of every matching table into the
    /// visitor as `(src_internal, far_label, nbr)` pairs — `far_label` is the
    /// table's owning dst label so callers can project internal endpoints to
    /// external ids even when the edge type's endpoint tags are unconstrained.
    /// Returns the resolved internal src id.
    pub fn visit_out_nbrs<F>(
        &self,
        edge_label: LabelId,
        src_label: LabelId,
        _dst_label: LabelId,
        src_id: VertexId,
        ts: Timestamp,
        mut f: F,
    ) -> Option<u32>
    where
        F: FnMut(u32, LabelId, HotNbr),
    {
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

        self.persistent.data_store.with_edge_tables(|edge_tables| {
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.src_label() == actual_src)
            {
                let far_label = table.dst_label();
                table.visit_out_with_gate(src_internal, ts, &gate, |nbr| {
                    f(src_internal, far_label, nbr)
                });
            }
        });
        Some(src_internal)
    }

    /// Visit raw in-edge neighbors of `dst` without building a vector.
    ///
    /// In-direction counterpart of `visit_out_nbrs`, streaming
    /// `(dst_internal, far_label, nbr)` pairs where `far_label` is the
    /// table's owning src label. Returns the resolved internal dst id.
    pub fn visit_in_nbrs<F>(
        &self,
        edge_label: LabelId,
        _src_label: LabelId,
        dst_label: LabelId,
        dst_id: VertexId,
        ts: Timestamp,
        mut f: F,
    ) -> Option<u32>
    where
        F: FnMut(u32, LabelId, HotNbr),
    {
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

        self.persistent.data_store.with_edge_tables(|edge_tables| {
            let gate = self.pending_gate();
            for table in edge_tables
                .values()
                .map(|arc| arc.read())
                .filter(|t| t.label() == edge_label && t.dst_label() == actual_dst)
            {
                let far_label = table.src_label();
                table.visit_in_with_gate(dst_internal, ts, &gate, |nbr| {
                    f(dst_internal, far_label, nbr)
                });
            }
        });
        Some(dst_internal)
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
                let tbl_dst = table.dst_label();
                for mut record in
                    table.out_edges_with_gate_projected(src_internal, ts, &gate, projection)
                {
                    record.dst_vid = endpoint_to_external(self, tbl_dst, record.dst_vid, ts);
                    records.push(record);
                }
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
                let tbl_src = table.src_label();
                for mut record in
                    table.in_edges_with_gate_projected(dst_internal, ts, &gate, projection)
                {
                    record.src_vid = endpoint_to_external(self, tbl_src, record.src_vid, ts);
                    records.push(record);
                }
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
                let tbl_dst = table.dst_label();
                for mut record in table.out_edges_with_gate_projected_limit(
                    src_internal,
                    ts,
                    &gate,
                    projection,
                    remaining,
                ) {
                    record.dst_vid = endpoint_to_external(self, tbl_dst, record.dst_vid, ts);
                    records.push(record);
                }
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
                let tbl_src = table.src_label();
                for mut record in table.in_edges_with_gate_projected_limit(
                    dst_internal,
                    ts,
                    &gate,
                    projection,
                    remaining,
                ) {
                    record.src_vid = endpoint_to_external(self, tbl_src, record.src_vid, ts);
                    records.push(record);
                }
            }
            records
        });
        Some(records)
    }
}

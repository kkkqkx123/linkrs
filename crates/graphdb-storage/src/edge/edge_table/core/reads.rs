//! Read-only paths: visibility, adjacency, point lookups and scans.

use super::super::super::{CsrBase, CsrShardSet, EdgeRecord, MutableCsrTrait, Nbr};
use super::EdgeStore;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::Value;

use super::super::iterator::EdgeTableScanIterator;

impl EdgeStore {
    /// Shared physical row location for point lookups: one physical topology
    /// address plus the authoritative MVCC check. Adjacency batches, full
    /// scans and point lookups all resolve rows through this routing instead
    /// of duplicating group arithmetic; scans additionally share
    /// [`EdgeStore::is_visible`] as the single visibility gate.
    pub(crate) fn physical_location(
        &self,
        csr: &CsrShardSet,
        src: u32,
        dst: VertexId,
    ) -> Option<Nbr> {
        csr.get_edge_physical(src, dst)
    }

    /// Single visibility gate for topology reads. Row stamps never decide
    /// visibility; only the version authority does.
    pub(crate) fn is_visible(&self, edge_id: EdgeId, ts: Timestamp) -> bool {
        self.mvcc.is_edge_visible(edge_id, ts)
    }

    /// Pending-aware variant of the shared visibility gate.
    pub(crate) fn is_visible_with_gate(
        &self,
        edge_id: EdgeId,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> bool {
        self.mvcc.is_edge_visible_with_gate(edge_id, ts, gate)
    }

    /// Fill a caller buffer with every visible neighbor of one row without
    /// internal allocation. Shared batch primitive behind the adjacency
    /// accessor and the allocating convenience wrappers below.
    pub(crate) fn fill_visible_into(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        csr.visit_physical(src, |nbr| {
            if self.is_visible(nbr.edge_id, ts) {
                out.push(nbr);
            }
            true
        });
    }

    /// Pending-aware variant of the shared batch fill.
    pub(crate) fn fill_visible_into_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        csr.visit_physical(src, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
            }
            true
        });
    }

    /// Single row-location entry for point lookups: physical topology lookup
    /// plus the authoritative MVCC visibility check. Adjacency, existence
    /// and record reads must funnel through here rather than reading CSR
    /// timestamps directly; row stamps exist only for collection. Scans all
    /// physical matches for the key so a delete-then-rebuild pair (old
    /// tombstone plus new live row sharing one endpoint key) resolves to the
    /// visible live row instead of the first physical slot.
    pub(crate) fn merged_get_edge(
        &self,
        csr: &CsrShardSet,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
    ) -> Option<Nbr> {
        let mut found = None;
        csr.visit_physical(src, |nbr| {
            if nbr.to_vertex_id() == dst && self.is_visible(nbr.edge_id, ts) {
                found = Some(nbr);
                false
            } else {
                true
            }
        });
        found
    }

    /// Allocating convenience over the shared batch fill. High-frequency
    /// traversal and batch queries use the reusable-buffer accessor instead;
    /// this wrapper stays for call sites where an owned vector is handier.
    pub(crate) fn merged_edges_of(&self, csr: &CsrShardSet, src: u32, ts: Timestamp) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_visible_into(csr, src, ts, &mut out);
        out
    }

    /// Allocating pending-aware convenience over the shared batch fill.
    fn merged_edges_of_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        let mut out = Vec::new();
        self.fill_visible_into_with_gate(csr, src, ts, gate, &mut out);
        out
    }

    /// First-`limit` visible neighbors without decoding the full adjacency.
    /// Uses the physical visit path so high-degree `LIMIT` queries stop
    /// after `k` visible neighbors instead of decoding every edge plus properties.
    pub fn merged_out_nbrs_with_limit(&self, src: u32, ts: Timestamp, limit: usize) -> Vec<Nbr> {
        self.merged_nbrs_with_limit(&self.out_csr, src, ts, limit)
    }

    /// First-`limit` visible in-neighbors, mirroring the out direction.
    pub fn merged_in_nbrs_with_limit(&self, dst: u32, ts: Timestamp, limit: usize) -> Vec<Nbr> {
        self.merged_nbrs_with_limit(&self.in_csr, dst, ts, limit)
    }

    fn merged_nbrs_with_limit(
        &self,
        csr: &CsrShardSet,
        vid: u32,
        ts: Timestamp,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        csr.visit_physical(vid, |nbr| {
            if self.is_visible(nbr.edge_id, ts) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn merged_out_nbrs_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.out_csr, src, ts, gate)
    }

    pub fn merged_in_nbrs_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<Nbr> {
        self.merged_edges_of_with_gate(&self.in_csr, dst, ts, gate)
    }

    pub fn merged_out_nbrs_with_gate_limit(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        self.out_csr.visit_physical(src, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn merged_in_nbrs_with_gate_limit(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        limit: usize,
    ) -> Vec<Nbr> {
        if limit == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(limit.min(32));
        self.in_csr.visit_physical(dst, |nbr| {
            if self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                out.push(nbr);
                out.len() < limit
            } else {
                true
            }
        });
        out
    }

    pub fn out_edges_with_gate(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        self.out_edges_with_gate_projected(src, ts, gate, None)
    }

    pub fn out_edges_with_gate_projected(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_out_nbrs_with_gate(src, ts, gate)
            .into_iter()
            .map(|nbr| {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn in_edges_with_gate(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        self.in_edges_with_gate_projected(dst, ts, gate, None)
    }

    pub fn in_edges_with_gate_projected(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        self.merged_in_nbrs_with_gate(dst, ts, gate)
            .into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn out_edges_with_gate_projected_limit(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
        limit: usize,
    ) -> Vec<EdgeRecord> {
        if !self.is_open || limit == 0 {
            return Vec::new();
        }
        self.merged_out_nbrs_with_gate_limit(src, ts, gate, limit)
            .into_iter()
            .map(|nbr| {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid,
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub fn in_edges_with_gate_projected_limit(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
        limit: usize,
    ) -> Vec<EdgeRecord> {
        if !self.is_open || limit == 0 {
            return Vec::new();
        }
        self.merged_in_nbrs_with_gate_limit(dst, ts, gate, limit)
            .into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    pub(crate) fn edge_record_from_nbr(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
    ) -> EdgeRecord {
        self.edge_record_from_nbr_projected(src, nbr, query_ts, None)
    }

    pub(crate) fn edge_record_from_nbr_projected(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> EdgeRecord {
        let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
        let rank = nbr.rank;
        let properties = self.properties_for_edge_projected(nbr.edge_id, query_ts, projection);
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties,
        }
    }

    pub(crate) fn properties_for_edge(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Vec<(String, Value)> {
        self.properties_for_edge_projected(edge_id, query_ts, None)
    }

    /// Topology-first property read: MVCC authority decides visibility,
    /// then only the projected columns are decoded. `None` decodes all
    /// columns, `Some(&[])` decodes none. Null-valued columns are filtered
    /// out, so callers cannot distinguish NULL from a missing column; the
    /// streaming cursor path shares this projection contract.
    fn properties_for_edge_projected(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        // MVCCManager is the single visibility authority. Row stamps exist
        // only for collection and must not decide query visibility here.
        if !self.is_visible(edge_id, query_ts) {
            return Vec::new();
        }
        // Snapshot read through the property version chain so an old reader
        // observes the before-image, not the latest write.
        self.properties
            .get_projected_physical_by_edge_id(edge_id, query_ts, projection)
            .map(|rows| {
                rows.into_iter()
                    .filter_map(|(name, value)| value.map(|v| (name, v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolve the edge id for `(src, dst, rank)` without decoding properties.
    ///
    /// Operation-layer point lookups use it to recheck the fetched record
    /// through the pending-aware gate
    /// (`MVCCManager::is_edge_visible_with_gate`).
    pub fn edge_id_of(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeId> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, src, dst_key, ts)
            .map(|nbr| nbr.edge_id)
    }

    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.merged_get_edge(&self.out_csr, src, dst_key, ts)?;
        let properties = self.properties_for_edge(nbr.edge_id, ts);

        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn get_edge_with_gate(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.physical_location(&self.out_csr, src, dst_key)?;
        if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
            return None;
        }
        let properties = self.properties_for_edge(nbr.edge_id, ts);
        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn get_edge_with_gate_projected(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Option<EdgeRecord> {
        if !self.is_open {
            return None;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        let nbr = self.physical_location(&self.out_csr, src, dst_key)?;
        if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
            return None;
        }
        let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);
        Some(EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank,
            properties,
        })
    }

    pub fn out_edges(&self, src: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        self.out_edges_projected(src, ts, None)
    }

    pub fn out_edges_projected(
        &self,
        src: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_out_nbrs(src, ts);

        nbrs.into_iter()
            .map(|nbr| self.edge_record_from_nbr_projected(src, nbr, ts, projection))
            .collect()
    }

    /// Raw out-edge neighbors of `src` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.out_csr, src, ts)
    }

    pub fn in_edges(&self, dst: u32, ts: Timestamp) -> Vec<EdgeRecord> {
        self.in_edges_projected(dst, ts, None)
    }

    pub fn in_edges_projected(
        &self,
        dst: u32,
        ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        let nbrs = self.merged_in_nbrs(dst, ts);

        nbrs.into_iter()
            .map(|nbr| {
                let src_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                let properties = self.properties_for_edge_projected(nbr.edge_id, ts, projection);

                EdgeRecord {
                    src_vid,
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                }
            })
            .collect()
    }

    /// Raw in-edge neighbors of `dst` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Allocating convenience; high-frequency paths use the batch accessor.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        self.merged_edges_of(&self.in_csr, dst, ts)
    }

    /// Batch adjacency accessor bound to one snapshot.
    ///
    /// Traversal and batch queries use the accessor with a caller buffer so
    /// peak memory stays proportional to the batch size instead of the row
    /// degree. The buffer is valid only for the snapshot the accessor was
    /// created with and must not be held across snapshots.
    pub fn batch_accessor(
        &self,
        outgoing: bool,
        ts: Timestamp,
    ) -> super::super::iterator::AdjacencyBatchAccessor<'_> {
        super::super::iterator::AdjacencyBatchAccessor::new(self, outgoing, ts)
    }

    pub fn has_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> bool {
        if !self.is_open {
            return false;
        }
        let dst_key = Self::edge_endpoint_key(dst, rank);
        self.merged_get_edge(&self.out_csr, src, dst_key, ts)
            .is_some()
    }

    pub fn edge_count(&self) -> u64 {
        self.out_csr.edge_count()
    }

    pub fn delta_edge_count(&self) -> u64 {
        self.out_csr.edge_count() + self.in_csr.edge_count()
    }

    pub fn scan(&self, ts: Timestamp) -> Vec<EdgeRecord> {
        self.scan_projected(ts, None)
    }

    pub fn scan_projected(
        &self,
        ts: Timestamp,
        projection: Option<Vec<String>>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }

        self.iter_projected(ts, projection).collect()
    }

    /// Batch full-table scan with a pending gate. Materializes every visible
    /// record, so it is an offline path for index builds and scatter-gather
    /// queries; latency-sensitive traversals should use the streaming
    /// [`EdgeStore::iter`] plus per-vertex limit pushdown instead.
    pub fn scan_with_gate(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let mut records = Vec::new();
        for (src_vid, nbr) in self.out_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_nbr(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
            ));
        }
        records
    }

    pub fn scan_with_gate_projected(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        let mut records = Vec::new();
        for (src_vid, nbr) in self.out_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_nbr_projected(
                src_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
                projection,
            ));
        }
        records
    }

    /// Optimizer-facing statistics snapshot for one property column
    /// (zone-map aggregated, with live row count).
    pub fn column_stats_snapshot(
        &self,
        column: &str,
    ) -> Option<crate::stats_reader::ColumnStatsSnapshot> {
        self.properties.column_stats_snapshot(column)
    }

    pub fn iter(&self, ts: Timestamp) -> EdgeTableScanIterator<'_> {
        EdgeTableScanIterator::new(self, ts)
    }

    pub fn iter_projected(
        &self,
        ts: Timestamp,
        projection: Option<Vec<String>>,
    ) -> EdgeTableScanIterator<'_> {
        EdgeTableScanIterator::with_projection(self, ts, projection)
    }
}

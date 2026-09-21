//! Read-only paths: visibility, adjacency, point lookups and scans.

use super::super::super::bundled_csr::decode_scalar;
use super::super::super::csr_shared::decode_endpoint_pair;
use super::super::super::{CsrBase, CsrShardSet, EdgeRecord, HotNbr, Nbr, RecordForm};
use super::EdgeStore;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::Value;

use super::super::iterator::EdgeTableScanIterator;

/// Query-time context for projecting the property payload of one edge record.
#[derive(Clone, Copy)]
struct PropertyQuery<'a> {
    query_ts: Timestamp,
    projection: Option<&'a [String]>,
    /// Selects the shard row holding the inline value for bundled tables.
    outgoing: bool,
}

impl EdgeStore {
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
        // Stage the physical row once, then compact it with a single
        // authority pass instead of one visibility call per edge.
        csr.visit_physical(src, |nbr| {
            out.push(nbr);
            true
        });
        self.mvcc.retain_visible(out, ts);
    }

    /// Visit every visible neighbor of one row without any allocation.
    ///
    /// Hot-only traversal primitive: no intermediate vector is built and the
    /// stamp lines are never touched, so per-vertex fan-out over thousands
    /// of vertices pays only topology bandwidth. The visitor runs inline on
    /// the physical walk.
    pub(crate) fn visit_visible_with_gate<F>(
        &self,
        csr: &CsrShardSet,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        mut f: F,
    ) where
        F: FnMut(HotNbr),
    {
        csr.visit_hot(src, |hot| {
            if self.is_visible_with_gate(hot.edge_id, ts, gate) {
                f(hot);
            }
            true
        });
    }

    /// Out-direction visit without allocation, for traversal fan-out.
    pub(crate) fn visit_out_with_gate<F>(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        f: F,
    ) where
        F: FnMut(HotNbr),
    {
        self.visit_visible_with_gate(&self.out_csr, src, ts, gate, f);
    }

    /// In-direction visit without allocation, for traversal fan-out.
    pub(crate) fn visit_in_with_gate<F>(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        f: F,
    ) where
        F: FnMut(HotNbr),
    {
        self.visit_visible_with_gate(&self.in_csr, dst, ts, gate, f);
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
        // Compare the packed key halves directly: building a `VertexId`
        // per slot just to compare it is pure overhead on this scan.
        let (endpoint, rank) = decode_endpoint_pair(dst);
        let mut found = None;
        csr.visit_physical(src, |nbr| {
            if nbr.endpoint == endpoint && nbr.rank == rank && self.is_visible(nbr.edge_id, ts) {
                found = Some(nbr);
                false
            } else {
                true
            }
        });
        found
    }

    /// Pending-aware merged point lookup: scans every physical generation for
    /// the endpoint key and returns the first one passing the pending gate.
    pub(crate) fn merged_get_edge_with_gate(
        &self,
        csr: &CsrShardSet,
        src: u32,
        dst: VertexId,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Option<Nbr> {
        let (endpoint, rank) = decode_endpoint_pair(dst);
        let mut found = None;
        csr.visit_physical(src, |nbr| {
            if nbr.endpoint == endpoint
                && nbr.rank == rank
                && self.is_visible_with_gate(nbr.edge_id, ts, gate)
            {
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
        // Hot-only stream with the pending gate: no cold-line touch, no
        // intermediate neighbor vector.
        let mut out = Vec::new();
        self.out_csr.visit_hot(src, |hot| {
            if self.is_visible_with_gate(hot.edge_id, ts, gate) {
                out.push(self.edge_record_from_hot_projected(
                    VertexId::from_int64(src as i64),
                    VertexId::from_int64(hot.endpoint as i64),
                    hot.rank,
                    hot.edge_id,
                    PropertyQuery {
                        query_ts: ts,
                        projection,
                        outgoing: true,
                    },
                ));
            }
            true
        });
        out
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
        // Hot-only stream mirroring the out direction with the pending gate.
        let mut out = Vec::new();
        self.in_csr.visit_hot(dst, |hot| {
            if self.is_visible_with_gate(hot.edge_id, ts, gate) {
                out.push(self.edge_record_from_hot_projected(
                    VertexId::from_int64(hot.endpoint as i64),
                    VertexId::from_int64(dst as i64),
                    hot.rank,
                    hot.edge_id,
                    PropertyQuery {
                        query_ts: ts,
                        projection,
                        outgoing: false,
                    },
                ));
            }
            true
        });
        out
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
                let properties = if self.is_bundled() {
                    self.bundled_properties_at(true, src, nbr.edge_id, ts, projection)
                } else {
                    self.properties_for_edge_projected_columnar(nbr.edge_id, ts, projection)
                };
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
                let properties = if self.is_bundled() {
                    self.bundled_properties_at(false, dst, nbr.edge_id, ts, projection)
                } else {
                    self.properties_for_edge_projected_columnar(nbr.edge_id, ts, projection)
                };
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
        // The neighbors above always come from the out direction (table
        // scans and out-row assembly), so the bundled fast path reads the
        // out shard row directly.
        let properties = if self.is_bundled() {
            self.bundled_properties_at(true, src, nbr.edge_id, query_ts, projection)
        } else {
            self.properties_for_edge_projected_columnar(nbr.edge_id, query_ts, projection)
        };
        let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
        let rank = nbr.rank;
        EdgeRecord {
            src_vid: VertexId::from_int64(src as i64),
            dst_vid,
            rank,
            properties,
        }
    }

    /// Assemble a record from a hot half without touching stamp lines.
    ///
    /// Hot-only counterpart of [`Self::edge_record_from_nbr_projected`]
    /// for record paths that stream topology and resolve visibility by
    /// edge id through the authority.
    fn edge_record_from_hot_projected(
        &self,
        src_vid: VertexId,
        dst_vid: VertexId,
        rank: i64,
        edge_id: EdgeId,
        query: PropertyQuery<'_>,
    ) -> EdgeRecord {
        let PropertyQuery {
            query_ts,
            projection,
            outgoing,
        } = query;
        let properties = if self.is_bundled() {
            let row = if outgoing {
                src_vid.as_int64().unwrap_or(0) as u32
            } else {
                dst_vid.as_int64().unwrap_or(0) as u32
            };
            self.bundled_properties_at(outgoing, row, edge_id, query_ts, projection)
        } else {
            self.properties_for_edge_projected_columnar(edge_id, query_ts, projection)
        };
        EdgeRecord {
            src_vid,
            dst_vid,
            rank,
            properties,
        }
    }

    /// Whether this table stores its single scalar inline in the CSR.
    pub(crate) fn is_bundled(&self) -> bool {
        self.schema.record_form == RecordForm::Bundled
    }

    /// Decode the inline value of one edge from its shard row.
    ///
    /// Returns an empty vector for invisible edges, NULL slots, pure
    /// topologies and projections excluding the single property, mirroring
    /// the columnar contract that NULL reads as absent.
    pub(crate) fn bundled_properties_at(
        &self,
        outgoing: bool,
        row: u32,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        if !self.is_visible(edge_id, query_ts) {
            return Vec::new();
        }
        let Some(prop) = self.schema.properties.first() else {
            return Vec::new();
        };
        if let Some(names) = projection {
            if !names.iter().any(|n| n == &prop.name) {
                return Vec::new();
            }
        }
        let shards = if outgoing {
            &self.out_csr
        } else {
            &self.in_csr
        };
        match shards.bundled_value_at(row, edge_id) {
            Some((raw, true)) => vec![(prop.name.clone(), decode_scalar(raw, &prop.data_type))],
            _ => Vec::new(),
        }
    }

    /// Edge-id-only inline read for call sites without a row address.
    ///
    /// Scans the out direction (which mirrors every bundled value) for the
    /// owning row. Rare-path fallback only; row-addressed reads use
    /// [`Self::bundled_properties_at`].
    pub(crate) fn bundled_scan_properties(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        if !self.is_visible(edge_id, query_ts) {
            return Vec::new();
        }
        for gid in self.out_csr.existing_group_ids() {
            let base = crate::edge::node_group::group_base(gid, self.out_csr.group_bits());
            let mut hit: Option<u32> = None;
            if let Some(variant) = self.out_csr.group_variant(gid) {
                for (local_vid, nbr) in variant.iter_all() {
                    if nbr.edge_id == edge_id {
                        hit = Some(base + local_vid.as_int64().unwrap_or(0) as u32);
                        break;
                    }
                }
            }
            if let Some(src) = hit {
                return self.bundled_properties_at(true, src, edge_id, query_ts, projection);
            }
        }
        Vec::new()
    }

    pub(crate) fn properties_for_edge(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Vec<(String, Value)> {
        self.properties_for_edge_projected(edge_id, query_ts, None)
    }

    /// Topology-first property read with the record-form dispatch.
    ///
    /// Bundled tables decode the inline value column (scanning for the
    /// owning row when the caller has no row address); every other form
    /// reads the columnar store.
    fn properties_for_edge_projected(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        if self.is_bundled() {
            return self.bundled_scan_properties(edge_id, query_ts, projection);
        }
        self.properties_for_edge_projected_columnar(edge_id, query_ts, projection)
    }

    /// Topology-first property read: MVCC authority decides visibility,
    /// then only the projected columns are decoded. `None` decodes all
    /// columns, `Some(&[])` decodes none. Null-valued columns are filtered
    /// out, so callers cannot distinguish NULL from a missing column; the
    /// streaming cursor path shares this projection contract.
    ///
    /// Columnar body behind the record-form dispatch above; bundled tables
    /// never reach here.
    fn properties_for_edge_projected_columnar(
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
        let properties = if self.is_bundled() {
            self.bundled_properties_at(true, src, nbr.edge_id, ts, None)
        } else {
            self.properties_for_edge(nbr.edge_id, ts)
        };

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
        let nbr = self.merged_get_edge_with_gate(&self.out_csr, src, dst_key, ts, gate)?;
        let properties = if self.is_bundled() {
            self.bundled_properties_at(true, src, nbr.edge_id, ts, None)
        } else {
            self.properties_for_edge(nbr.edge_id, ts)
        };
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
        let nbr = self.merged_get_edge_with_gate(&self.out_csr, src, dst_key, ts, gate)?;
        let properties = if self.is_bundled() {
            self.bundled_properties_at(true, src, nbr.edge_id, ts, projection)
        } else {
            self.properties_for_edge_projected_columnar(nbr.edge_id, ts, projection)
        };
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

        // Hot-only stream: visibility resolves by edge id through the
        // authority, so the cold stamp lines stay out of cache and no
        // intermediate neighbor vector is built.
        let mut out = Vec::new();
        self.out_csr.visit_hot(src, |hot| {
            if self.is_visible(hot.edge_id, ts) {
                out.push(self.edge_record_from_hot_projected(
                    VertexId::from_int64(src as i64),
                    VertexId::from_int64(hot.endpoint as i64),
                    hot.rank,
                    hot.edge_id,
                    PropertyQuery {
                        query_ts: ts,
                        projection,
                        outgoing: true,
                    },
                ));
            }
            true
        });
        out
    }

    /// Raw out-edge neighbors of `src` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Owned single-row convenience over [`Self::fill_visible_into`].
    /// Record-building paths stream hot-only without this intermediate, and
    /// high-frequency traversals use `visit_out_with_gate` or the batch
    /// accessor instead.
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

        // Hot-only stream mirroring the out direction: no cold-line touch,
        // no intermediate neighbor vector.
        let mut out = Vec::new();
        self.in_csr.visit_hot(dst, |hot| {
            if self.is_visible(hot.edge_id, ts) {
                out.push(self.edge_record_from_hot_projected(
                    VertexId::from_int64(hot.endpoint as i64),
                    VertexId::from_int64(dst as i64),
                    hot.rank,
                    hot.edge_id,
                    PropertyQuery {
                        query_ts: ts,
                        projection,
                        outgoing: false,
                    },
                ));
            }
            true
        });
        out
    }

    /// Raw in-edge neighbors of `dst` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Owned single-row convenience over [`Self::fill_visible_into`].
    /// Record-building paths stream hot-only without this intermediate, and
    /// high-frequency traversals use `visit_in_with_gate` or the batch
    /// accessor instead.
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

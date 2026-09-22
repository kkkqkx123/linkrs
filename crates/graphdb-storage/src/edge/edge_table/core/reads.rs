//! Read-only paths: visibility, adjacency, point lookups and scans.

use super::super::super::bundled_csr::decode_scalar;
use super::super::super::csr_shared::decode_endpoint_pair;
use super::super::super::{CsrBase, CsrShardSet, EdgeRecord, HotNbr, Nbr, RecordForm};
use super::super::staging::EdgeStagingBatch;
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
    /// Single-direction tables without the out leg report empty: the leg
    /// stores nothing, so callers observe an empty adjacency, not an error.
    pub(crate) fn visit_out_with_gate<F>(
        &self,
        src: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        f: F,
    ) where
        F: FnMut(HotNbr),
    {
        if !self.schema.has_out() {
            log::debug!(
                "visit_out on table '{}' without out leg: empty adjacency",
                self.label_name
            );
            return;
        }
        self.visit_visible_with_gate(&self.out_csr, src, ts, gate, f);
    }

    /// In-direction visit without allocation, for traversal fan-out.
    /// Mirrors the out leg: a missing in leg reads as empty adjacency.
    pub(crate) fn visit_in_with_gate<F>(
        &self,
        dst: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        f: F,
    ) where
        F: FnMut(HotNbr),
    {
        if !self.schema.has_in() {
            log::debug!(
                "visit_in on table '{}' without in leg: empty adjacency",
                self.label_name
            );
            return;
        }
        self.visit_visible_with_gate(&self.in_csr, dst, ts, gate, f);
    }

    /// Collect visible hot neighbors of one row through the pending gate.
    ///
    /// Staging primitive behind the projected adjacency paths: topology
    /// streams once, the authority decides per edge, and the caller decodes
    /// properties in batch afterwards instead of interleaving one decode
    /// per edge into the walk. Survivor order follows the row walk.
    fn collect_visible_hots_with_gate(
        &self,
        csr: &CsrShardSet,
        vid: u32,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        out: &mut Vec<HotNbr>,
    ) {
        out.clear();
        csr.visit_hot(vid, |hot| {
            if self.is_visible_with_gate(hot.edge_id, ts, gate) {
                out.push(hot);
            }
            true
        });
    }

    /// Collect visible hot neighbors of one row without a pending gate.
    ///
    /// Plain-authority counterpart of
    /// [`Self::collect_visible_hots_with_gate`] with the same ordering and
    /// verdict contract.
    fn collect_visible_hots(
        &self,
        csr: &CsrShardSet,
        vid: u32,
        ts: Timestamp,
        out: &mut Vec<HotNbr>,
    ) {
        out.clear();
        csr.visit_hot(vid, |hot| {
            if self.is_visible(hot.edge_id, ts) {
                out.push(hot);
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
        if !self.schema.has_out() {
            return Vec::new();
        }
        self.merged_nbrs_with_limit(&self.out_csr, src, ts, limit)
    }

    /// First-`limit` visible in-neighbors, mirroring the out direction.
    pub fn merged_in_nbrs_with_limit(&self, dst: u32, ts: Timestamp, limit: usize) -> Vec<Nbr> {
        if !self.schema.has_in() {
            return Vec::new();
        }
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
        if limit == 0 || !self.schema.has_out() {
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
        if limit == 0 || !self.schema.has_in() {
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
        if !self.is_open || !self.schema.has_out() {
            return Vec::new();
        }
        // Hot-only stream with the pending gate: no cold-line touch. The
        // walk only stages visible edges; properties decode in one batch
        // afterwards so the projection resolves its columns once. Each
        // record reuses the gate verdict above instead of re-deciding
        // without the gate.
        let mut hots = Vec::new();
        self.collect_visible_hots_with_gate(&self.out_csr, src, ts, gate, &mut hots);
        if self.is_bundled() {
            return hots
                .into_iter()
                .map(|hot| {
                    self.edge_record_from_hot_projected_assume_visible(
                        VertexId::from_int64(src as i64),
                        VertexId::from_int64(hot.endpoint as i64),
                        hot.rank,
                        hot.edge_id,
                        PropertyQuery {
                            query_ts: ts,
                            projection,
                            outgoing: true,
                        },
                    )
                })
                .collect();
        }
        let ids: Vec<EdgeId> = hots.iter().map(|hot| hot.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        hots.into_iter()
            .zip(decoded)
            .map(|(hot, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(src as i64),
                dst_vid: VertexId::from_int64(hot.endpoint as i64),
                rank: hot.rank,
                properties,
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
        if !self.is_open || !self.schema.has_in() {
            return Vec::new();
        }
        // Hot-only stream mirroring the out direction with the pending gate.
        // The walk only stages visible edges; properties decode in one batch
        // afterwards. Each record reuses the gate verdict above instead of
        // re-deciding without the gate.
        let mut hots = Vec::new();
        self.collect_visible_hots_with_gate(&self.in_csr, dst, ts, gate, &mut hots);
        if self.is_bundled() {
            return hots
                .into_iter()
                .map(|hot| {
                    self.edge_record_from_hot_projected_assume_visible(
                        VertexId::from_int64(hot.endpoint as i64),
                        VertexId::from_int64(dst as i64),
                        hot.rank,
                        hot.edge_id,
                        PropertyQuery {
                            query_ts: ts,
                            projection,
                            outgoing: false,
                        },
                    )
                })
                .collect();
        }
        let ids: Vec<EdgeId> = hots.iter().map(|hot| hot.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        hots.into_iter()
            .zip(decoded)
            .map(|(hot, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(hot.endpoint as i64),
                dst_vid: VertexId::from_int64(dst as i64),
                rank: hot.rank,
                properties,
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
        let nbrs = self.merged_out_nbrs_with_gate_limit(src, ts, gate, limit);
        if self.is_bundled() {
            return nbrs
                .into_iter()
                .map(|nbr| {
                    // Neighbors above passed the gate: decode without re-deciding.
                    let properties = self.bundled_properties_at_assume_visible(
                        true,
                        src,
                        nbr.edge_id,
                        ts,
                        projection,
                    );
                    EdgeRecord {
                        src_vid: VertexId::from_int64(src as i64),
                        dst_vid: VertexId::from_int64(nbr.endpoint as i64),
                        rank: nbr.rank,
                        properties,
                    }
                })
                .collect();
        }
        let ids: Vec<EdgeId> = nbrs.iter().map(|nbr| nbr.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        nbrs.into_iter()
            .zip(decoded)
            .map(|(nbr, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(src as i64),
                dst_vid: VertexId::from_int64(nbr.endpoint as i64),
                rank: nbr.rank,
                properties,
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
        let nbrs = self.merged_in_nbrs_with_gate_limit(dst, ts, gate, limit);
        if self.is_bundled() {
            return nbrs
                .into_iter()
                .map(|nbr| {
                    // Neighbors above passed the gate: decode without re-deciding.
                    let properties = self.bundled_properties_at_assume_visible(
                        false,
                        dst,
                        nbr.edge_id,
                        ts,
                        projection,
                    );
                    EdgeRecord {
                        src_vid: VertexId::from_int64(nbr.endpoint as i64),
                        dst_vid: VertexId::from_int64(dst as i64),
                        rank: nbr.rank,
                        properties,
                    }
                })
                .collect();
        }
        let ids: Vec<EdgeId> = nbrs.iter().map(|nbr| nbr.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        nbrs.into_iter()
            .zip(decoded)
            .map(|(nbr, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(nbr.endpoint as i64),
                dst_vid: VertexId::from_int64(dst as i64),
                rank: nbr.rank,
                properties,
            })
            .collect()
    }

    /// Whether this table stores its single scalar inline in the CSR.
    pub(crate) fn is_bundled(&self) -> bool {
        self.schema.record_form == RecordForm::Bundled
    }

    /// Decode the inline value of one edge from its shard row.
    ///
    /// Paired value half of the bundled read: topology walks yield edge ids,
    /// this entry resolves the inline value for one of them. Returns an
    /// empty vector for invisible edges, NULL slots, pure topologies and
    /// projections excluding the single property, mirroring the columnar
    /// contract that NULL reads as absent.
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

    /// Visibility-assumed property reads: the caller already resolved
    /// visibility through the version authority, so these entries decode
    /// without a second authority lookup. Every adjacency, point and scan
    /// path that filters first must use these; the checking entries above
    /// stay for direct callers without a prior verdict.
    pub(crate) fn bundled_properties_at_assume_visible(
        &self,
        outgoing: bool,
        row: u32,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        let _ = query_ts;
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

    /// Edge-id-only inline read without a visibility recheck.
    ///
    /// Caller must hold a prior verdict; the row scan below only locates
    /// the owning row, it never decides visibility.
    pub(crate) fn bundled_scan_properties_assume_visible(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
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
                return self.bundled_properties_at_assume_visible(
                    true, src, edge_id, query_ts, projection,
                );
            }
        }
        Vec::new()
    }

    /// Columnar projection without a visibility recheck.
    ///
    /// Caller must hold a prior authority verdict; the store read below is
    /// purely physical (snapshot decode through the version chain).
    pub(crate) fn properties_for_edge_projected_columnar_assume_visible(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, Value)> {
        self.properties
            .get_projected_physical_by_edge_id(edge_id, query_ts, projection)
            .map(|rows| {
                rows.into_iter()
                    .filter_map(|(name, value)| value.map(|v| (name, v)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Columnar batch projection without a visibility recheck.
    ///
    /// Batch form of
    /// [`Self::properties_for_edge_projected_columnar_assume_visible`]:
    /// the caller still holds one authority verdict per edge; this entry
    /// only replaces N per-edge decodes with one shared column mapping.
    /// Output order follows the input; unmapped edges decode to empty,
    /// mirroring the single-edge contract.
    pub(crate) fn properties_for_edge_projected_columnar_batch_assume_visible(
        &self,
        edge_ids: &[EdgeId],
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<Vec<(String, Value)>> {
        self.properties
            .get_projected_physical_batch_by_edge_ids(edge_ids, query_ts, projection)
            .into_iter()
            .map(|rows| {
                rows.map(|pairs| {
                    pairs
                        .into_iter()
                        .filter_map(|(name, value)| value.map(|v| (name, v)))
                        .collect()
                })
                .unwrap_or_default()
            })
            .collect()
    }

    /// Hot record assembly without a visibility recheck.
    ///
    /// For streams that already filtered through the authority (or its
    /// pending gate): reusing that verdict also keeps gate semantics intact,
    /// since a fresh check here would re-decide without the gate.
    fn edge_record_from_hot_projected_assume_visible(
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
            self.bundled_properties_at_assume_visible(outgoing, row, edge_id, query_ts, projection)
        } else {
            self.properties_for_edge_projected_columnar_assume_visible(
                edge_id, query_ts, projection,
            )
        };
        EdgeRecord {
            src_vid,
            dst_vid,
            rank,
            properties,
        }
    }

    /// Out-row record assembly without a visibility recheck.
    pub(crate) fn edge_record_from_nbr_projected_assume_visible(
        &self,
        src: u32,
        nbr: Nbr,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> EdgeRecord {
        let properties = if self.is_bundled() {
            self.bundled_properties_at_assume_visible(true, src, nbr.edge_id, query_ts, projection)
        } else {
            self.properties_for_edge_projected_columnar_assume_visible(
                nbr.edge_id,
                query_ts,
                projection,
            )
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

    /// In-row record assembly without a visibility recheck.
    pub(crate) fn edge_record_from_in_nbr_assume_visible(
        &self,
        dst: u32,
        nbr: Nbr,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> EdgeRecord {
        let properties = if self.is_bundled() {
            self.bundled_properties_at_assume_visible(false, dst, nbr.edge_id, query_ts, projection)
        } else {
            self.properties_for_edge_projected_columnar_assume_visible(
                nbr.edge_id,
                query_ts,
                projection,
            )
        };
        EdgeRecord {
            src_vid: VertexId::from_int64(nbr.endpoint as i64),
            dst_vid: VertexId::from_int64(dst as i64),
            rank: nbr.rank,
            properties,
        }
    }

    /// Resolve the edge id for `(src, dst, rank)` without decoding properties.
    ///
    /// Operation-layer point lookups use it to recheck the fetched record
    /// through the pending-aware gate
    /// (`MVCCManager::is_edge_visible_with_gate`). Single-direction tables
    /// without the out leg probe the stored in leg at the destination row,
    /// mirroring the delete path, instead of reporting a missing edge.
    pub fn edge_id_of(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeId> {
        if !self.is_open {
            return None;
        }
        if self.schema.has_out() {
            let dst_key = Self::edge_endpoint_key(dst, rank);
            return self
                .merged_get_edge(&self.out_csr, src, dst_key, ts)
                .map(|nbr| nbr.edge_id);
        }
        if self.schema.has_in() {
            log::debug!(
                "edge_id_of on table '{}' without out leg: probing stored in leg",
                self.label_name
            );
            let src_key = Self::edge_endpoint_key(src, rank);
            return self
                .merged_get_edge(&self.in_csr, dst, src_key, ts)
                .map(|nbr| nbr.edge_id);
        }
        None
    }

    /// Point lookup resolving through the stored leg.
    ///
    /// Mirrors `edge_id_of`: tables without the out leg answer from the in
    /// leg so single-direction tables stay queryable instead of silent.
    pub fn get_edge(&self, src: u32, dst: u32, rank: i64, ts: Timestamp) -> Option<EdgeRecord> {
        self.get_edge_with_id(src, dst, rank, ts)
            .map(|(record, _)| record)
    }

    /// Point lookup resolving through the stored leg, paired with the edge id.
    ///
    /// Single fused entry for operation-layer point lookups: one physical
    /// row scan plus the authoritative visibility verdict inside
    /// `merged_get_edge`, with the property projection reusing that verdict
    /// instead of deciding visibility a second time. Callers needing a
    /// pending-aware recheck reuse the returned id instead of scanning the
    /// row again through `edge_id_of`.
    pub fn get_edge_with_id(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> Option<(EdgeRecord, EdgeId)> {
        if !self.is_open {
            return None;
        }

        if self.schema.has_out() {
            let dst_key = Self::edge_endpoint_key(dst, rank);
            let nbr = self.merged_get_edge(&self.out_csr, src, dst_key, ts)?;
            // The merged lookup above already passed the authority: decode
            // without a second verdict.
            let properties = if self.is_bundled() {
                self.bundled_properties_at_assume_visible(true, src, nbr.edge_id, ts, None)
            } else {
                self.properties_for_edge_projected_columnar_assume_visible(nbr.edge_id, ts, None)
            };

            return Some((
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                },
                nbr.edge_id,
            ));
        }
        if self.schema.has_in() {
            let src_key = Self::edge_endpoint_key(src, rank);
            let nbr = self.merged_get_edge(&self.in_csr, dst, src_key, ts)?;
            let properties = if self.is_bundled() {
                self.bundled_properties_at_assume_visible(false, dst, nbr.edge_id, ts, None)
            } else {
                self.properties_for_edge_projected_columnar_assume_visible(nbr.edge_id, ts, None)
            };
            return Some((
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                },
                nbr.edge_id,
            ));
        }
        None
    }

    pub fn get_edge_with_gate(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Option<EdgeRecord> {
        self.get_edge_projected_with_id(src, dst, rank, ts, gate, None)
            .map(|(record, _)| record)
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
        self.get_edge_projected_with_id(src, dst, rank, ts, gate, projection)
            .map(|(record, _)| record)
    }

    /// Pending-aware point lookup paired with the edge id.
    ///
    /// Fused counterpart of `get_edge_with_id` for gate-carrying callers:
    /// one physical row scan through `merged_get_edge_with_gate`, with the
    /// projection reusing the gate verdict instead of deciding visibility a
    /// second time. Operation-layer rechecks reuse the returned id for the
    /// authority gate and the deletion-stamp probe instead of scanning the
    /// row again.
    pub fn get_edge_projected_with_id(
        &self,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
        projection: Option<&[String]>,
    ) -> Option<(EdgeRecord, EdgeId)> {
        if !self.is_open {
            return None;
        }
        if self.schema.has_out() {
            let dst_key = Self::edge_endpoint_key(dst, rank);
            let nbr = self.merged_get_edge_with_gate(&self.out_csr, src, dst_key, ts, gate)?;
            // The gate verdict above is reused: the checking entries would
            // otherwise re-decide without the gate.
            let properties = if self.is_bundled() {
                self.bundled_properties_at_assume_visible(true, src, nbr.edge_id, ts, projection)
            } else {
                self.properties_for_edge_projected_columnar_assume_visible(
                    nbr.edge_id,
                    ts,
                    projection,
                )
            };
            return Some((
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                },
                nbr.edge_id,
            ));
        }
        if self.schema.has_in() {
            let src_key = Self::edge_endpoint_key(src, rank);
            let nbr = self.merged_get_edge_with_gate(&self.in_csr, dst, src_key, ts, gate)?;
            let properties = if self.is_bundled() {
                self.bundled_properties_at_assume_visible(false, dst, nbr.edge_id, ts, projection)
            } else {
                self.properties_for_edge_projected_columnar_assume_visible(
                    nbr.edge_id,
                    ts,
                    projection,
                )
            };
            return Some((
                EdgeRecord {
                    src_vid: VertexId::from_int64(src as i64),
                    dst_vid: VertexId::from_int64(dst as i64),
                    rank,
                    properties,
                },
                nbr.edge_id,
            ));
        }
        None
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
        if !self.schema.has_out() {
            log::debug!(
                "out_edges on table '{}' without out leg: empty adjacency",
                self.label_name
            );
            return Vec::new();
        }

        // Hot-only stream: visibility resolves by edge id through the
        // authority, so the cold stamp lines stay out of cache. The walk
        // only stages visible edges; properties decode in one batch
        // afterwards. Each record reuses the verdict above instead of
        // querying the authority twice per edge.
        let mut hots = Vec::new();
        self.collect_visible_hots(&self.out_csr, src, ts, &mut hots);
        if self.is_bundled() {
            return hots
                .into_iter()
                .map(|hot| {
                    self.edge_record_from_hot_projected_assume_visible(
                        VertexId::from_int64(src as i64),
                        VertexId::from_int64(hot.endpoint as i64),
                        hot.rank,
                        hot.edge_id,
                        PropertyQuery {
                            query_ts: ts,
                            projection,
                            outgoing: true,
                        },
                    )
                })
                .collect();
        }
        let ids: Vec<EdgeId> = hots.iter().map(|hot| hot.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        hots.into_iter()
            .zip(decoded)
            .map(|(hot, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(src as i64),
                dst_vid: VertexId::from_int64(hot.endpoint as i64),
                rank: hot.rank,
                properties,
            })
            .collect()
    }

    /// Raw out-edge neighbors of `src` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Owned single-row convenience over [`Self::fill_visible_into`].
    /// Record-building paths stream hot-only without this intermediate, and
    /// high-frequency traversals use `visit_out_with_gate` or the batch
    /// accessor instead.
    pub fn merged_out_nbrs(&self, src: u32, ts: Timestamp) -> Vec<Nbr> {
        if !self.schema.has_out() {
            return Vec::new();
        }
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
        if !self.schema.has_in() {
            log::debug!(
                "in_edges on table '{}' without in leg: empty adjacency",
                self.label_name
            );
            return Vec::new();
        }

        // Hot-only stream mirroring the out direction: no cold-line touch.
        // The walk only stages visible edges; properties decode in one batch
        // afterwards. Each record reuses the verdict above instead of
        // querying the authority twice per edge.
        let mut hots = Vec::new();
        self.collect_visible_hots(&self.in_csr, dst, ts, &mut hots);
        if self.is_bundled() {
            return hots
                .into_iter()
                .map(|hot| {
                    self.edge_record_from_hot_projected_assume_visible(
                        VertexId::from_int64(hot.endpoint as i64),
                        VertexId::from_int64(dst as i64),
                        hot.rank,
                        hot.edge_id,
                        PropertyQuery {
                            query_ts: ts,
                            projection,
                            outgoing: false,
                        },
                    )
                })
                .collect();
        }
        let ids: Vec<EdgeId> = hots.iter().map(|hot| hot.edge_id).collect();
        let decoded =
            self.properties_for_edge_projected_columnar_batch_assume_visible(&ids, ts, projection);
        hots.into_iter()
            .zip(decoded)
            .map(|(hot, properties)| EdgeRecord {
                src_vid: VertexId::from_int64(hot.endpoint as i64),
                dst_vid: VertexId::from_int64(dst as i64),
                rank: hot.rank,
                properties,
            })
            .collect()
    }

    /// Raw in-edge neighbors of `dst` (MVCC-filtered, snapshot-consistent)
    /// with no property decoding.
    ///
    /// Owned single-row convenience over [`Self::fill_visible_into`].
    /// Record-building paths stream hot-only without this intermediate, and
    /// high-frequency traversals use `visit_in_with_gate` or the batch
    /// accessor instead.
    pub fn merged_in_nbrs(&self, dst: u32, ts: Timestamp) -> Vec<Nbr> {
        if !self.schema.has_in() {
            return Vec::new();
        }
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
        self.edge_id_of(src, dst, rank, ts).is_some()
    }

    /// Live edge count on the stored leg: the out leg when stored, else the
    /// in leg. Single-direction tables report their one leg, never zero.
    pub fn edge_count(&self) -> u64 {
        if self.schema.has_out() {
            self.out_csr.edge_count()
        } else {
            self.in_csr.edge_count()
        }
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
    ///
    /// Scans the stored leg: tables without the out leg iterate the in leg
    /// with swapped endpoint assembly so single-direction tables stay
    /// scannable instead of silent.
    pub fn scan_with_gate(
        &self,
        ts: Timestamp,
        gate: &crate::mvcc_visibility::PendingGate<'_>,
    ) -> Vec<EdgeRecord> {
        if !self.is_open {
            return Vec::new();
        }
        if self.schema.has_out() {
            let mut records = Vec::new();
            for (src_vid, nbr) in self.out_csr.iter_all() {
                if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                    continue;
                }
                // Gate verdict reused: the checking assembly would
                // otherwise re-decide without the gate.
                records.push(self.edge_record_from_nbr_projected_assume_visible(
                    src_vid.as_int64().unwrap_or(0) as u32,
                    nbr,
                    ts,
                    None,
                ));
            }
            return records;
        }
        let mut records = Vec::new();
        for (dst_vid, nbr) in self.in_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_in_nbr_assume_visible(
                dst_vid.as_int64().unwrap_or(0) as u32,
                nbr,
                ts,
                None,
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
        if self.schema.has_out() {
            let mut records = Vec::new();
            for (src_vid, nbr) in self.out_csr.iter_all() {
                if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                    continue;
                }
                // Gate verdict reused: the checking assembly would
                // otherwise re-decide without the gate.
                records.push(self.edge_record_from_nbr_projected_assume_visible(
                    src_vid.as_int64().unwrap_or(0) as u32,
                    nbr,
                    ts,
                    projection,
                ));
            }
            return records;
        }
        let mut records = Vec::new();
        for (dst_vid, nbr) in self.in_csr.iter_all() {
            if !self.is_visible_with_gate(nbr.edge_id, ts, gate) {
                continue;
            }
            records.push(self.edge_record_from_in_nbr_assume_visible(
                dst_vid.as_int64().unwrap_or(0) as u32,
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

    /// Owner-view existence check overlaying a caller-owned staging batch.
    ///
    /// Default reads stay on the committed snapshot for isolation; this
    /// helper lets the batch owner observe read-your-writes without
    /// touching committed state. Empty batches delegate to the fast path.
    pub fn has_edge_with_batch(
        &self,
        batch: &EdgeStagingBatch,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> bool {
        if batch.is_empty() {
            return self.has_edge(src, dst, rank, ts);
        }
        if batch.contains_insert(src, dst, rank) {
            return true;
        }
        if batch.contains_delete(src, dst, rank) {
            return false;
        }
        self.has_edge(src, dst, rank, ts)
    }

    /// Owner-view point lookup overlaying a caller-owned staging batch.
    ///
    /// Net inserts synthesize a record from staged properties (uncommitted
    /// edges own no edge id yet); net deletes hide the committed edge.
    pub fn get_edge_with_batch(
        &self,
        batch: &EdgeStagingBatch,
        src: u32,
        dst: u32,
        rank: i64,
        ts: Timestamp,
    ) -> Option<EdgeRecord> {
        if batch.is_empty() {
            return self.get_edge(src, dst, rank, ts);
        }
        if batch.contains_delete(src, dst, rank) {
            return None;
        }
        if let Some(staged) = batch
            .staged_inserts()
            .iter()
            .rev()
            .find(|ins| ins.src == src && ins.dst == dst && ins.rank == rank)
        {
            return Some(EdgeRecord {
                src_vid: VertexId::from_int64(src as i64),
                dst_vid: VertexId::from_int64(dst as i64),
                rank,
                properties: staged.properties.clone(),
            });
        }
        self.get_edge(src, dst, rank, ts)
    }

    /// Owner-view out adjacency overlaying a caller-owned staging batch.
    pub fn out_edges_with_batch(
        &self,
        src: u32,
        ts: Timestamp,
        batch: &EdgeStagingBatch,
    ) -> Vec<EdgeRecord> {
        if batch.is_empty() {
            return self.out_edges(src, ts);
        }
        let mut out: Vec<EdgeRecord> = self
            .out_edges(src, ts)
            .into_iter()
            .filter(|record| {
                let dst = record.dst_vid.as_int64().unwrap_or(-1) as u32;
                !batch.contains_delete(src, dst, record.rank)
            })
            .collect();
        for ins in batch.staged_inserts() {
            if ins.src != src || batch.contains_delete(ins.src, ins.dst, ins.rank) {
                continue;
            }
            let dst = ins.dst;
            let rank = ins.rank;
            if out.iter().any(|record| {
                record.dst_vid.as_int64().unwrap_or(-1) as u32 == dst && record.rank == rank
            }) {
                continue;
            }
            out.push(EdgeRecord {
                src_vid: VertexId::from_int64(ins.src as i64),
                dst_vid: VertexId::from_int64(ins.dst as i64),
                rank: ins.rank,
                properties: ins.properties.clone(),
            });
        }
        out
    }

    /// Owner-view in adjacency overlaying a caller-owned staging batch.
    pub fn in_edges_with_batch(
        &self,
        dst: u32,
        ts: Timestamp,
        batch: &EdgeStagingBatch,
    ) -> Vec<EdgeRecord> {
        if batch.is_empty() {
            return self.in_edges(dst, ts);
        }
        let mut out: Vec<EdgeRecord> = self
            .in_edges(dst, ts)
            .into_iter()
            .filter(|record| {
                let src = record.src_vid.as_int64().unwrap_or(-1) as u32;
                !batch.contains_delete(src, dst, record.rank)
            })
            .collect();
        for ins in batch.staged_inserts() {
            if ins.dst != dst || batch.contains_delete(ins.src, ins.dst, ins.rank) {
                continue;
            }
            let src = ins.src;
            let rank = ins.rank;
            if out.iter().any(|record| {
                record.src_vid.as_int64().unwrap_or(-1) as u32 == src && record.rank == rank
            }) {
                continue;
            }
            out.push(EdgeRecord {
                src_vid: VertexId::from_int64(ins.src as i64),
                dst_vid: VertexId::from_int64(ins.dst as i64),
                rank: ins.rank,
                properties: ins.properties.clone(),
            });
        }
        out
    }
}

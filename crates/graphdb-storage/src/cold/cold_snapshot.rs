use std::collections::HashMap;

use crate::cold::cold_persistence::build_presence_bitmap;
use crate::cold::cold_property_index::ColdPropertyIndex;
use crate::edge::edge_table::remap::remap_immutable_csr;
use crate::edge::{Csr, CsrBase, CsrWithProperties, EdgeRecord, EdgeSchema, Nbr};
use graphdb_core::types::{LabelId, Timestamp, VertexId};
use graphdb_core::{StorageResult, Value};

/// A scanned edge record from a ColdSnapshot.
/// Contains enough information to construct a full Edge.
#[derive(Debug, Clone)]
pub struct ColdEdgeRecord {
    pub src_internal: u32,
    pub dst_vid: VertexId,
    pub nbr: Nbr,
    pub rank: i64,
    pub properties: Option<Vec<(String, Value)>>,
}

/// Read-only single-file columnar snapshot for cold analytics queries.
///
/// Contains CSR adjacency data (out/in), columnar property storage,
/// snapshot metadata, edge schema, and an optional ordered property index.
///
/// File format:
/// ```text
/// [4]  Magic "LKCS"
/// [4]  Version (u32 LE, 1)
/// [8]  Snapshot timestamp (u64 LE)
/// [8]  Edge count (u64 LE)
/// [4]  Label ID (u32 LE)
/// [8]  Vertex capacity (u64 LE)
/// --- sections ---
/// [8]  Out CSR length (u64 LE) + [N] out CSR (marker + raw/dict payload)
/// [8]  In CSR length (u64 LE) + [N] in CSR (marker + raw/dict payload)
/// [8]  Property table length (u64 LE) + [N] property data
///      (1-byte marker: 0x00 raw, 0x01 zstd-compressed)
/// [8]  Schema length (u64 LE) + [N] Schema data (JSON)
/// [8]  Index length (u64 LE, 0 = no index)
/// [N]  Index data (ColdPropertyIndex::encode)
/// [8]  Presence bitmap length (u64 LE, 0 = none)
/// [N]  Presence bitmap (u64 words, bit v = row v has out edges)
/// [4]  CRC32 of all preceding bytes
/// ```
///
/// Dict-encoded CSR payload:
/// ```text
/// [8]  Vertex capacity (u64 LE)
/// [8]  Edge count (u64 LE)
/// [8]  Dict size (u64 LE)
/// [N]  Dict entries: [1+len] vertex id bytes each
/// [8]  Offsets length (u64 LE)
/// [N]  Offsets (u32 LE, vertex_capacity + 1 entries)
/// [N]  Edge records: (dict_id u32, edge_id u64, ts u64)
/// ```
#[derive(Debug, Clone)]
pub struct ColdSnapshot {
    pub(crate) snapshot_ts: Timestamp,
    pub(crate) label: LabelId,
    pub(crate) edge_count: u64,
    pub(crate) vertex_capacity: usize,
    pub(crate) out_csr: Csr,
    pub(crate) in_csr: Csr,
    pub(crate) properties: CsrWithProperties,
    pub(crate) schema: EdgeSchema,
    pub(crate) property_index: Option<ColdPropertyIndex>,
    /// Bit v set = row v holds at least one out edge. Lets full scans skip
    /// empty rows without touching the CSR offsets.
    pub(crate) vertex_presence: Option<Vec<u64>>,
    /// Backing `.lkcs` file when opened from or created at a path.
    pub(crate) path: Option<std::path::PathBuf>,
}

impl ColdSnapshot {
    pub fn snapshot_ts(&self) -> Timestamp {
        self.snapshot_ts
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    pub fn edge_count(&self) -> u64 {
        self.edge_count
    }

    pub fn vertex_capacity(&self) -> usize {
        self.vertex_capacity
    }

    pub fn out_csr(&self) -> &Csr {
        &self.out_csr
    }

    pub fn in_csr(&self) -> &Csr {
        &self.in_csr
    }

    pub fn properties(&self) -> &CsrWithProperties {
        &self.properties
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub fn property_index(&self) -> Option<&ColdPropertyIndex> {
        self.property_index.as_ref()
    }

    /// Whether this snapshot carries a property index at all.
    pub fn has_property_index(&self) -> bool {
        self.property_index.as_ref().is_some_and(|i| !i.is_empty())
    }

    pub fn get_out_edges(&self, src: u32) -> Vec<Nbr> {
        self.out_csr
            .edges_of(src)
            .iter()
            .map(|e| Nbr::new(e.endpoint, e.rank, e.edge_id))
            .collect()
    }

    pub fn get_in_edges(&self, dst: u32) -> Vec<Nbr> {
        self.in_csr
            .edges_of(dst)
            .iter()
            .map(|e| Nbr::new(e.endpoint, e.rank, e.edge_id))
            .collect()
    }

    pub fn get_edge(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        self.out_csr
            .get_edge(src, dst)
            .map(|e| Nbr::new(e.endpoint, e.rank, e.edge_id))
    }

    /// Find an edge from `src` (internal CSR index) to `dst` (internal vertex id).
    ///
    /// The CSR stores neighbors as encoded `(dst_internal, rank)` keys, so the
    /// lookup decodes each neighbor before comparing with `dst`.
    pub fn get_edge_to_dst(&self, src: u32, dst: u32) -> Option<Nbr> {
        self.out_csr.edges_of(src).iter().find_map(|e| {
            let decoded = VertexId::from_int64(e.endpoint as i64);
            if decoded.as_int64() == Some(dst as i64) {
                Some(Nbr::new(e.endpoint, e.rank, e.edge_id))
            } else {
                None
            }
        })
    }

    pub fn degree(&self, src: u32) -> usize {
        self.out_csr.edges_of(src).len()
    }

    pub fn scan_edges(&self) -> Vec<ColdEdgeRecord> {
        let cap = self.vertex_capacity;
        let mut results = Vec::with_capacity(self.edge_count as usize);
        for src in 0..cap {
            if !self.row_has_edges(src) {
                continue;
            }
            let src_u32 = src as u32;
            for nbr in self.out_csr.edges_of(src_u32) {
                let dst_vid = VertexId::from_int64(nbr.endpoint as i64);
                let rank = nbr.rank;
                results.push(ColdEdgeRecord {
                    src_internal: src as u32,
                    dst_vid,
                    nbr: Nbr::new(nbr.endpoint, nbr.rank, nbr.edge_id),
                    rank,
                    properties: None,
                });
            }
        }
        results
    }

    /// Whether vertex row `row` holds at least one out edge, using the
    /// presence bitmap when available and the CSR offsets otherwise.
    pub fn row_has_edges(&self, row: usize) -> bool {
        if let Some(bitmap) = &self.vertex_presence {
            let word = row / 64;
            let bit = row % 64;
            bitmap.get(word).is_some_and(|w| (w & (1u64 << bit)) != 0)
        } else {
            !self.out_csr.edges_of(row as u32).is_empty()
        }
    }

    /// The out-row presence bitmap (bit v = row v has edges), when the
    /// snapshot file carried one.
    pub fn vertex_presence(&self) -> Option<&[u64]> {
        self.vertex_presence.as_deref()
    }

    pub fn nbr_to_edge_record(
        &self,
        nbr: &Nbr,
        src_vid: VertexId,
        dst_vid: VertexId,
    ) -> EdgeRecord {
        let rank = nbr.rank;
        let properties = self
            .properties
            .read_properties_by_edge_id(nbr.edge_id)
            .unwrap_or_default();
        EdgeRecord {
            src_vid,
            dst_vid,
            rank,
            properties,
        }
    }

    /// Rebuild both CSRs with translated rows/neighbors and a truncated row
    /// space (max edge-bearing row + 1) after a vertex compaction remap.
    ///
    /// `src_mapping` applies to out rows / in neighbors; `dst_mapping` to in
    /// rows / out neighbors (per-label internal ID spaces).
    ///
    /// Only the in-memory snapshot is updated; the backing `.lkcs` file is
    /// left untouched and should be re-exported to stay consistent.
    pub fn remap_vertex_ids(
        &mut self,
        src_mapping: Option<&HashMap<u32, u32>>,
        dst_mapping: Option<&HashMap<u32, u32>>,
    ) -> StorageResult<()> {
        let new_out = remap_immutable_csr(&self.out_csr, src_mapping, dst_mapping)?;
        let new_in = remap_immutable_csr(&self.in_csr, dst_mapping, src_mapping)?;

        let out_capacity = new_out.vertex_capacity();
        let in_capacity = new_in.vertex_capacity();
        self.out_csr = new_out;
        self.in_csr = new_in;
        self.vertex_capacity = out_capacity.max(in_capacity);
        self.edge_count = self.out_csr.edge_count();
        let bitmap = build_presence_bitmap(&self.out_csr);
        self.vertex_presence = if bitmap.is_empty() {
            None
        } else {
            Some(bitmap)
        };

        if let Some(index) = self.property_index.as_mut() {
            index.remap_vertex_ids(src_mapping, dst_mapping);
        }

        log::debug!(
            "ColdSnapshot[label={}] remapped vertex IDs (src_mapping={}, dst_mapping={}); capacity={}, edges={}",
            self.label,
            src_mapping.map(|m| m.len()).unwrap_or(0),
            dst_mapping.map(|m| m.len()).unwrap_or(0),
            self.vertex_capacity,
            self.edge_count
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cold::cold_persistence::{decode_csr_dict, encode_csr_dict};
    use crate::cold::ColdPropertyIndex;
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::edge::{EdgeSchema, EdgeStrategy};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::EdgeId;
    use graphdb_core::Value;

    fn make_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_cold_snapshot_roundtrip() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .unwrap();

        let exported = table.export_snapshot(100).unwrap();
        assert_eq!(exported.out_csr.edge_count(), 2);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");

        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.edge_count(), 2);
        assert_eq!(snapshot.snapshot_ts(), 100);
        assert_eq!(snapshot.label(), 0);

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.edge_count(), 2);
        assert_eq!(loaded.snapshot_ts(), 100);
        assert_eq!(loaded.get_out_edges(0).len(), 2);
        assert_eq!(loaded.get_out_edges(1).len(), 0);
    }

    fn make_edge_key(endpoint: u32, rank: i64) -> VertexId {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&(endpoint as i64).to_be_bytes());
        data.extend_from_slice(&rank.to_be_bytes());
        VertexId::from_bytes(data)
    }

    #[test]
    fn test_cold_snapshot_get_edge() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 50)
            .unwrap();
        table
            .insert_edge(5, 10, 0, &[("weight".to_string(), Value::Double(3.0))], 150)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(200).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let edge = snapshot.get_edge(0, make_edge_key(1, 0));
        assert!(edge.is_some());
        assert_eq!(edge.unwrap().edge_id, EdgeId(0));
        assert!(snapshot.get_edge(0, make_edge_key(99, 0)).is_none());

        assert_eq!(snapshot.degree(5), 1);
        assert_eq!(snapshot.degree(0), 1);
    }

    #[test]
    fn test_cold_snapshot_integrity() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        ColdSnapshot::create(&exported, &path).unwrap();

        let mut corrupted = std::fs::read(&path).unwrap();
        if let Some(b) = corrupted.last_mut() {
            *b ^= 0xFF;
        }
        std::fs::write(&path, &corrupted).unwrap();

        let result = ColdSnapshot::open(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_cold_snapshot_vertex_capacity() {
        let mut table = make_table();
        for i in 0..10u32 {
            table
                .insert_edge(
                    i,
                    i + 1,
                    0,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert!(snapshot.vertex_capacity() >= 10);
    }

    #[test]
    fn test_cold_snapshot_scan_edges() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();
        table
            .insert_edge(3, 4, 0, &[("weight".to_string(), Value::Double(3.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let scanned = snapshot.scan_edges();
        assert_eq!(scanned.len(), 3);

        let keys: Vec<(u32, VertexId)> = scanned
            .iter()
            .map(|r| (r.src_internal, r.dst_vid))
            .collect();
        assert!(keys.contains(&(0, VertexId::from_int64(1))));
        assert!(keys.contains(&(0, VertexId::from_int64(2))));
        assert!(keys.contains(&(3, VertexId::from_int64(4))));
    }

    #[test]
    fn test_cold_snapshot_get_edge_to_dst() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 7, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 3, &[("weight".to_string(), Value::Double(2.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let nbr = snapshot.get_edge_to_dst(0, 1).expect("edge 0->1 exists");
        let decoded_dst = VertexId::from_int64(nbr.endpoint as i64);
        let decoded_rank = nbr.rank;
        assert_eq!(decoded_dst, VertexId::from_int64(1));
        assert_eq!(decoded_rank, 7);
        assert!(snapshot.get_edge_to_dst(0, 99).is_none());
        assert!(snapshot.get_edge_to_dst(1, 1).is_none());
    }

    #[test]
    fn test_cold_snapshot_nbr_to_edge_record_rank() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 42, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let nbr = snapshot.get_edge_to_dst(0, 1).expect("edge exists");
        let record =
            snapshot.nbr_to_edge_record(&nbr, VertexId::from_int64(0), VertexId::from_int64(1));
        assert_eq!(record.rank, 42);
    }

    #[test]
    fn test_cold_snapshot_nbr_to_edge_record() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(42.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        // Query edge and convert to record
        if let Some(nbr) = snapshot.get_edge(0, make_edge_key(1, 0)) {
            let src_vid = VertexId::from_int64(0);
            let dst_vid = make_edge_key(1, 0);
            let record = snapshot.nbr_to_edge_record(&nbr, src_vid, dst_vid);

            assert_eq!(record.src_vid, src_vid);
            assert_eq!(record.dst_vid, dst_vid);
            assert!(!record.properties.is_empty());
            assert_eq!(record.properties[0].0, "weight");
            assert_eq!(record.properties[0].1, Value::Double(42.0));
        } else {
            panic!("expected edge to exist");
        }
    }

    #[test]
    fn test_cold_snapshot_scan_edges_empty() {
        let table = make_table();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let scanned = snapshot.scan_edges();
        assert!(scanned.is_empty());
    }

    #[test]
    fn test_cold_snapshot_remap_vertex_ids() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.vertex_capacity(), 6);
        assert_eq!(snapshot.edge_count(), 4);

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();

        assert_eq!(snapshot.vertex_capacity(), 5);
        assert_eq!(snapshot.edge_count(), 4);
        assert_eq!(snapshot.get_out_edges(3).len(), 1);
        assert_eq!(snapshot.get_out_edges(4).len(), 1);
        assert_eq!(snapshot.get_out_edges(5).len(), 0);
        assert_eq!(snapshot.get_in_edges(3).len(), 2);
        assert_eq!(snapshot.get_in_edges(4).len(), 2);
        assert_eq!(snapshot.degree(3), 1);
    }

    #[test]
    fn test_cold_snapshot_remap_persists_to_file() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert_eq!(snapshot.vertex_capacity(), 6);

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();
        snapshot.persist().unwrap();

        // A reload from the rewritten file must observe the remap.
        let reloaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(reloaded.vertex_capacity(), 5);
        assert_eq!(reloaded.edge_count(), 4);
        assert_eq!(reloaded.get_out_edges(3).len(), 1);
        assert_eq!(reloaded.get_out_edges(4).len(), 1);
        assert_eq!(reloaded.get_out_edges(5).len(), 0);
        assert_eq!(reloaded.get_in_edges(3).len(), 2);
        assert_eq!(reloaded.get_in_edges(4).len(), 2);
        assert_eq!(reloaded.degree(3), 1);
    }

    #[test]
    fn test_cold_snapshot_remap_dst_only() {
        let mut table = make_table();
        for (src, dst) in [(0u32, 4u32), (2, 5), (4, 5), (5, 4)] {
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let mut snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let mapping: HashMap<u32, u32> = [(4, 3), (5, 4)].into_iter().collect();
        snapshot.remap_vertex_ids(None, Some(&mapping)).unwrap();

        assert_eq!(snapshot.vertex_capacity(), 6);
        assert_eq!(snapshot.edge_count(), 4);
        assert_eq!(snapshot.get_out_edges(4).len(), 1);
        assert_eq!(snapshot.get_out_edges(5).len(), 1);
        assert_eq!(snapshot.get_in_edges(3).len(), 2);
        assert_eq!(snapshot.get_in_edges(4).len(), 2);
        assert_eq!(snapshot.get_in_edges(5).len(), 0);
    }

    #[test]
    fn test_cold_snapshot_property_index_build_and_lookup() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .unwrap();
        table
            .insert_edge(3, 4, 0, &[("weight".to_string(), Value::Double(3.5))], 100)
            .unwrap();

        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        assert!(!index.is_empty());
        assert_eq!(index.indexed_property_names(), vec!["weight".to_string()]);

        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        // Equality lookup: weight = 2.5
        let key = codec.encode(&Value::Double(2.5)).unwrap();
        let upper = graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key);
        let hits = index.lookup("weight", &key, &upper);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 2);
        assert_eq!(hits[0].rank, 0);

        // Range lookup: weight >= 2.5
        let hits = index.lookup("weight", &key, &[0xFF]);
        assert_eq!(hits.len(), 2);

        // Unknown property yields nothing
        assert!(index.lookup("missing", &key, &[0xFF]).is_empty());
    }

    #[test]
    fn test_cold_snapshot_property_index_roundtrip() {
        let mut table = make_table();
        table
            .insert_edge(0, 1, 5, &[("weight".to_string(), Value::Double(9.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        let snapshot = ColdSnapshot::create_with_index(&exported, Some(index), &path).unwrap();
        assert!(snapshot.has_property_index());

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert!(loaded.has_property_index());
        let index = loaded.property_index().unwrap();
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let key = codec.encode(&Value::Double(9.0)).unwrap();
        let hits = index.lookup(
            "weight",
            &key,
            &graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 1);
        assert_eq!(hits[0].rank, 5);
    }

    #[test]
    fn test_cold_snapshot_property_index_remap() {
        let mut table = make_table();
        table
            .insert_edge(0, 5, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let index = ColdPropertyIndex::build(&exported, &["weight".to_string()]);
        let mut snapshot = ColdSnapshot::create_with_index(&exported, Some(index), &path).unwrap();

        let mapping: HashMap<u32, u32> = [(5, 3)].into_iter().collect();
        snapshot
            .remap_vertex_ids(Some(&mapping), Some(&mapping))
            .unwrap();
        snapshot.persist().unwrap();

        let reloaded = ColdSnapshot::open(&path).unwrap();
        let index = reloaded.property_index().unwrap();
        let codec = graphdb_core::value::ordered_codec::OrderedCodec::new();
        let key = codec.encode(&Value::Double(1.0)).unwrap();
        let hits = index.lookup(
            "weight",
            &key,
            &graphdb_core::value::ordered_codec::OrderedCodec::prefix_upper_bound(&key),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].src_internal, 0);
        assert_eq!(hits[0].dst_internal, 3);
    }

    #[test]
    fn test_property_read_by_edge_id() {
        use crate::edge::CsrWithProperties;
        use crate::edge::property_schema::PropertySchema;

        let schemas = vec![
            PropertySchema::new("name".to_string(), 0, graphdb_core::DataType::String),
            PropertySchema::new("age".to_string(), 1, graphdb_core::DataType::Int),
        ];
        let mut pt = CsrWithProperties::new(1, schemas);

        let edge_id = graphdb_core::types::EdgeId(42);
        let offset = pt
            .insert_for_edge(
                edge_id,
                &[
                    ("name".to_string(), Value::String("Alice".into())),
                    ("age".to_string(), Value::Int(30)),
                ],
                100,
            )
            .unwrap();

        let props = pt.read_properties_by_edge_id(edge_id).unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, "name");
        assert_eq!(props[0].1, Value::String("Alice".into()));
        assert_eq!(props[1].0, "age");
        assert_eq!(props[1].1, Value::Int(30));

        // Missing edge_id returns None
        assert!(pt.read_properties_by_edge_id(graphdb_core::types::EdgeId(999)).is_none());
    }

    #[test]
    fn test_cold_snapshot_presence_bitmap() {
        let mut table = make_table();
        for src in [0u32, 5, 100] {
            table
                .insert_edge(
                    src,
                    src + 1,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();

        let presence = snapshot.vertex_presence().expect("presence bitmap present");
        assert!(snapshot.row_has_edges(0));
        assert!(snapshot.row_has_edges(5));
        assert!(snapshot.row_has_edges(100));
        assert!(!snapshot.row_has_edges(1));
        assert!(!snapshot.row_has_edges(200));

        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.vertex_presence(), Some(presence));
        assert_eq!(loaded.scan_edges().len(), 3);
        assert!(loaded.row_has_edges(5));
        assert!(!loaded.row_has_edges(6));
    }

    #[test]
    fn test_cold_snapshot_dict_encoding_smaller() {
        // 100 sources x 5 destinations = 500 edges over 6 distinct endpoints:
        // dict encoding (24 B/edge + tiny dict) beats raw (37 B/edge).
        let mut table = make_table();
        for src in 0..100u32 {
            for dst in 0..5u32 {
                table
                    .insert_edge(
                        src,
                        dst,
                        0,
                        &[("weight".to_string(), Value::Double(1.0))],
                        100,
                    )
                    .unwrap();
            }
        }
        let csr = table.export_snapshot(100).unwrap().out_csr;
        let raw = csr.dump();
        let dict = encode_csr_dict(&csr);
        assert!(
            dict.len() < raw.len(),
            "dict encoding should shrink repetitive CSRs: dict={}, raw={}",
            dict.len(),
            raw.len()
        );

        let roundtrip = decode_csr_dict(&dict).unwrap();
        assert_eq!(roundtrip.edge_count(), csr.edge_count());
        assert_eq!(roundtrip.edges_of(0).len(), 5);
        assert_eq!(roundtrip.edges_of(99).len(), 5);
        assert_eq!(roundtrip.edges_of(50).len(), 5);
        // Endpoint bytes survive the dict round-trip.
        assert_eq!(
            roundtrip.edges_of(0)[0].to_vertex_id(),
            csr.edges_of(0)[0].to_vertex_id()
        );
    }

    #[test]
    fn test_cold_snapshot_zstd_property_section() {
        // Large string-heavy property payloads must compress.
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "name".to_string(),
                graphdb_core::types::DataType::String,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();
        for i in 0..50u32 {
            table
                .insert_edge(
                    i,
                    i + 1,
                    0,
                    &[(
                        "name".to_string(),
                        Value::string(format!("repeated-pattern-{}", i % 7)),
                    )],
                    100,
                )
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        ColdSnapshot::create(&exported, &path).unwrap();
        let loaded = ColdSnapshot::open(&path).unwrap();
        assert_eq!(loaded.edge_count(), 50);
        let nbr = loaded.get_out_edges(10)[0];
        let props = loaded.properties().read_properties_by_edge_id(nbr.edge_id).unwrap();
        assert_eq!(props.len(), 1);
        assert_eq!(props[0].1, Value::string("repeated-pattern-3"));
    }
}

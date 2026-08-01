use std::collections::HashMap;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::{StorageError, StorageResult, Value};
use crate::storage::edge::edge_table::core::TimeTravelEdgeStore;
use crate::storage::edge::edge_table::remap::remap_immutable_csr;
use crate::storage::edge::{Csr, CsrBase, EdgeRecord, EdgeSchema, Nbr, PropertyTable};

use super::super::edge::edge_table::snapshot::ExportedEdgeSnapshot;

pub const COLD_SNAPSHOT_MAGIC: [u8; 4] = *b"LKCS";
pub const COLD_SNAPSHOT_VERSION: u32 = 1;
const HEADER_SIZE: usize = 36;

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
/// snapshot metadata, and edge schema.
///
/// File format:
/// ```text
/// [4]  Magic "LKCS"
/// [4]  Version (u32 LE)
/// [8]  Snapshot timestamp (u64 LE)
/// [8]  Edge count (u64 LE)
/// [4]  Label ID (u32 LE)
/// [8]  Vertex capacity (u64 LE)
/// --- sections ---
/// [8]  Out CSR length (u64 LE) + [N] Out CSR data
/// [8]  In CSR length (u64 LE) + [N] In CSR data
/// [8]  Property table length (u64 LE) + [N] Property table data
/// [8]  Schema length (u64 LE) + [N] Schema data (JSON)
/// [4]  CRC32 of all preceding bytes
/// ```
#[derive(Clone)]
pub struct ColdSnapshot {
    snapshot_ts: Timestamp,
    label: LabelId,
    edge_count: u64,
    vertex_capacity: usize,
    out_csr: Csr,
    in_csr: Csr,
    properties: PropertyTable,
    schema: EdgeSchema,
    /// Backing `.lkcs` file when opened from or created at a path.
    path: Option<PathBuf>,
}

impl ColdSnapshot {
    pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let file = std::fs::File::open(path.as_ref())
            .map_err(|e| StorageError::io_error(format!("failed to open snapshot: {}", e)))?;
        let mmap = unsafe {
            Mmap::map(&file)
                .map_err(|e| StorageError::io_error(format!("failed to mmap snapshot: {}", e)))?
        };
        let mut snapshot = Self::from_bytes(&mmap)?;
        snapshot.path = Some(path.as_ref().to_path_buf());
        drop(file);
        Ok(snapshot)
    }

    pub fn from_bytes(data: &[u8]) -> StorageResult<Self> {
        if data.len() < HEADER_SIZE + 4 {
            return Err(StorageError::deserialize_error(format!(
                "ColdSnapshot data too short: {} bytes",
                data.len()
            )));
        }

        let mut pos = 0usize;

        let magic = &data[pos..pos + 4];
        pos += 4;
        if magic != COLD_SNAPSHOT_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "invalid magic: {:?}",
                magic
            )));
        }

        let version = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        if version != COLD_SNAPSHOT_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported ColdSnapshot version: {}",
                version
            )));
        }

        let snapshot_ts = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let edge_count = u64::from_le_bytes(read_arr::<8>(data, &mut pos));
        let label = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        let vertex_capacity = u64::from_le_bytes(read_arr::<8>(data, &mut pos)) as usize;

        // Sections
        let out_data = read_section(data, &mut pos)?;
        let in_data = read_section(data, &mut pos)?;
        let prop_data = read_section(data, &mut pos)?;
        let schema_data = read_section(data, &mut pos)?;

        // CRC32 verification
        let stored_crc = u32::from_le_bytes(read_arr::<4>(data, &mut pos));
        let computed_crc = crc32fast::hash(&data[..pos - 4]);
        if stored_crc != computed_crc {
            return Err(StorageError::deserialize_error(format!(
                "CRC mismatch: stored={:#x}, computed={:#x}",
                stored_crc, computed_crc
            )));
        }

        // Deserialize
        let mut out_csr = Csr::new();
        out_csr.load(out_data)?;

        let mut in_csr = Csr::new();
        in_csr.load(in_data)?;

        let mut properties = PropertyTable::new();
        properties.load(prop_data)?;

        let schema_json = std::str::from_utf8(schema_data)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema utf-8: {}", e)))?;
        let schema: EdgeSchema = serde_json::from_str(schema_json)
            .map_err(|e| StorageError::deserialize_error(format!("invalid schema json: {}", e)))?;

        Ok(Self {
            snapshot_ts,
            label,
            edge_count,
            vertex_capacity,
            out_csr,
            in_csr,
            properties,
            schema,
            path: None,
        })
    }

    pub fn create<P: AsRef<Path>>(exported: &ExportedEdgeSnapshot, path: P) -> StorageResult<Self> {
        let buf = encode_snapshot(
            &exported.out_csr,
            &exported.in_csr,
            &exported.properties,
            &exported.schema,
            exported.snapshot_ts,
            exported.label,
        )?;

        std::fs::write(path.as_ref(), &buf)
            .map_err(|e| StorageError::io_error(format!("failed to write snapshot file: {}", e)))?;

        let mut snapshot = Self::from_bytes(&buf)?;
        snapshot.path = Some(path.as_ref().to_path_buf());
        Ok(snapshot)
    }

    /// Persist the current in-memory state back to the backing `.lkcs` file.
    ///
    /// Keeps the file consistent after an in-memory remap (e.g. vertex
    /// compaction). The file is a rebuildable cache, so a failure here only
    /// degrades to a stale file, never to incorrect queries.
    pub fn persist(&self) -> StorageResult<()> {
        let path = self.path.as_ref().ok_or_else(|| {
            StorageError::io_error("cold snapshot has no backing file to persist to")
        })?;
        let buf = encode_snapshot(
            &self.out_csr,
            &self.in_csr,
            &self.properties,
            &self.schema,
            self.snapshot_ts,
            self.label,
        )?;
        std::fs::write(path, &buf)
            .map_err(|e| StorageError::io_error(format!("failed to rewrite snapshot file: {}", e)))
    }

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

    pub fn properties(&self) -> &PropertyTable {
        &self.properties
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    pub fn get_out_edges(&self, src: u32) -> Vec<Nbr> {
        self.out_csr
            .edges_of(src)
            .iter()
            .map(|e| Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp))
            .collect()
    }

    pub fn get_in_edges(&self, dst: u32) -> Vec<Nbr> {
        self.in_csr
            .edges_of(dst)
            .iter()
            .map(|e| Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp))
            .collect()
    }

    pub fn get_edge(&self, src: u32, dst: VertexId) -> Option<Nbr> {
        self.out_csr
            .get_edge(src, dst)
            .map(|e| Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp))
    }

    /// Find an edge from `src` (internal CSR index) to `dst` (internal vertex id).
    ///
    /// The CSR stores neighbors as encoded `(dst_internal, rank)` keys, so the
    /// lookup decodes each neighbor before comparing with `dst`.
    pub fn get_edge_to_dst(&self, src: u32, dst: u32) -> Option<Nbr> {
        self.out_csr.edges_of(src).iter().find_map(|e| {
            let (decoded, _) = TimeTravelEdgeStore::decode_edge_endpoint(e.neighbor);
            if decoded.as_int64() == Some(dst as i64) {
                Some(Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp))
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
            let src_u32 = src as u32;
            for nbr in self.out_csr.edges_of(src_u32) {
                let (dst_vid, rank) = TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
                results.push(ColdEdgeRecord {
                    src_internal: src as u32,
                    dst_vid,
                    nbr: Nbr::new(nbr.neighbor, nbr.edge_id, nbr.prop_offset, nbr.timestamp),
                    rank,
                    properties: None,
                });
            }
        }
        results
    }

    pub fn nbr_to_edge_record(
        &self,
        nbr: &Nbr,
        src_vid: VertexId,
        dst_vid: VertexId,
    ) -> EdgeRecord {
        let (_, rank) = TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
        let properties = self
            .properties
            .read_properties(nbr.prop_offset)
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

fn read_arr<const N: usize>(data: &[u8], pos: &mut usize) -> [u8; N] {
    let arr: [u8; N] = data[*pos..*pos + N].try_into().unwrap();
    *pos += N;
    arr
}

fn read_section<'a>(data: &'a [u8], pos: &mut usize) -> StorageResult<&'a [u8]> {
    if *pos + 8 > data.len() {
        return Err(StorageError::deserialize_error(
            "unexpected end of section length",
        ));
    }
    let len = u64::from_le_bytes(read_arr::<8>(data, pos)) as usize;
    if *pos + len > data.len() {
        return Err(StorageError::deserialize_error(format!(
            "section data exceeds file: offset={}, len={}, file_size={}",
            *pos,
            len,
            data.len()
        )));
    }
    let section = &data[*pos..*pos + len];
    *pos += len;
    Ok(section)
}

fn write_section(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(data);
}

/// Serialize CSR/property data into the `.lkcs` file layout (header + sections + CRC).
fn encode_snapshot(
    out_csr: &Csr,
    in_csr: &Csr,
    properties: &PropertyTable,
    schema: &EdgeSchema,
    snapshot_ts: Timestamp,
    label: LabelId,
) -> StorageResult<Vec<u8>> {
    let out_data = out_csr.dump();
    let in_data = in_csr.dump();
    let prop_data = properties.dump();
    let schema_json =
        serde_json::to_string(schema).map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let schema_bytes = schema_json.as_bytes();

    let mut buf = Vec::new();

    buf.extend_from_slice(&COLD_SNAPSHOT_MAGIC);
    buf.extend_from_slice(&COLD_SNAPSHOT_VERSION.to_le_bytes());
    buf.extend_from_slice(&snapshot_ts.to_le_bytes());
    buf.extend_from_slice(&out_csr.edge_count().to_le_bytes());
    buf.extend_from_slice(&label.to_le_bytes());
    buf.extend_from_slice(&(out_csr.vertex_capacity() as u64).to_le_bytes());

    write_section(&mut buf, &out_data);
    write_section(&mut buf, &in_data);
    write_section(&mut buf, &prop_data);
    write_section(&mut buf, schema_bytes);

    let checksum = crc32fast::hash(&buf);
    buf.extend_from_slice(&checksum.to_le_bytes());

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::EdgeId;
    use crate::core::Value;
    use crate::storage::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::storage::edge::{EdgeSchema, EdgeStrategy};
    use crate::storage::types::StoragePropertyDef;

    fn make_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
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
        let (decoded_dst, decoded_rank) = TimeTravelEdgeStore::decode_edge_endpoint(nbr.neighbor);
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
    fn test_property_table_read_properties() {
        use crate::storage::edge::PropertyTable;

        let mut pt = PropertyTable::new();
        pt.add_property("name".to_string(), crate::core::DataType::String, false)
            .unwrap();
        pt.add_property("age".to_string(), crate::core::DataType::Int, false)
            .unwrap();

        let offset = pt
            .insert(
                &[
                    ("name".to_string(), Value::String("Alice".into())),
                    ("age".to_string(), Value::Int(30)),
                ],
                100,
            )
            .unwrap();

        let props = pt.read_properties(offset).unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, "name");
        assert_eq!(props[0].1, Value::String("Alice".into()));
        assert_eq!(props[1].0, "age");
        assert_eq!(props[1].1, Value::Int(30));

        // Missing offset returns None
        assert!(pt.read_properties(u32::MAX).is_none());
    }
}

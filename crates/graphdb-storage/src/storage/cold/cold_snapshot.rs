use std::path::Path;

use memmap2::Mmap;

use crate::core::types::{LabelId, Timestamp, VertexId};
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::{Csr, CsrBase, EdgeSchema, Nbr, PropertyTable};

use super::super::edge::edge_table::snapshot::ExportedEdgeSnapshot;

pub const COLD_SNAPSHOT_MAGIC: [u8; 4] = *b"LKCS";
pub const COLD_SNAPSHOT_VERSION: u32 = 1;
const HEADER_SIZE: usize = 36;

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
pub struct ColdSnapshot {
    snapshot_ts: Timestamp,
    label: LabelId,
    edge_count: u64,
    vertex_capacity: usize,
    out_csr: Csr,
    in_csr: Csr,
    properties: PropertyTable,
    schema: EdgeSchema,
}

impl ColdSnapshot {
    pub fn open<P: AsRef<Path>>(path: P) -> StorageResult<Self> {
        let file = std::fs::File::open(path.as_ref())
            .map_err(|e| StorageError::io_error(format!("failed to open snapshot: {}", e)))?;
        let mmap = unsafe {
            Mmap::map(&file)
                .map_err(|e| StorageError::io_error(format!("failed to mmap snapshot: {}", e)))?
        };
        let snapshot = Self::from_bytes(&mmap)?;
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
        })
    }

    pub fn create<P: AsRef<Path>>(exported: &ExportedEdgeSnapshot, path: P) -> StorageResult<Self> {
        let out_data = exported.out_csr.dump();
        let in_data = exported.in_csr.dump();
        let prop_data = exported.properties.dump();
        let schema_json = serde_json::to_string(&exported.schema)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        let schema_bytes = schema_json.as_bytes();

        let mut buf = Vec::new();

        buf.extend_from_slice(&COLD_SNAPSHOT_MAGIC);
        buf.extend_from_slice(&COLD_SNAPSHOT_VERSION.to_le_bytes());
        buf.extend_from_slice(&exported.snapshot_ts.to_le_bytes());
        buf.extend_from_slice(&exported.out_csr.edge_count().to_le_bytes());
        buf.extend_from_slice(&exported.label.to_le_bytes());
        buf.extend_from_slice(&(exported.out_csr.vertex_capacity() as u64).to_le_bytes());

        write_section(&mut buf, &out_data);
        write_section(&mut buf, &in_data);
        write_section(&mut buf, &prop_data);
        write_section(&mut buf, schema_bytes);

        let checksum = crc32fast::hash(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());

        std::fs::write(path.as_ref(), &buf)
            .map_err(|e| StorageError::io_error(format!("failed to write snapshot file: {}", e)))?;

        Self::from_bytes(&buf)
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
        self.out_csr.get_edge(src, dst).map(|e| {
            Nbr::new(e.neighbor, e.edge_id, e.prop_offset, e.timestamp)
        })
    }

    pub fn degree(&self, src: u32) -> usize {
        self.out_csr.edges_of(src).len()
    }
}

fn read_arr<const N: usize>(data: &[u8], pos: &mut usize) -> [u8; N] {
    let arr: [u8; N] = data[*pos..*pos + N].try_into().unwrap();
    *pos += N;
    arr
}

fn read_section<'a>(data: &'a [u8], pos: &mut usize) -> StorageResult<&'a [u8]> {
    if *pos + 8 > data.len() {
        return Err(StorageError::deserialize_error("unexpected end of section length"));
    }
    let len = u64::from_le_bytes(read_arr::<8>(data, pos)) as usize;
    if *pos + len > data.len() {
        return Err(StorageError::deserialize_error(format!(
            "section data exceeds file: offset={}, len={}, file_size={}",
            *pos, len, data.len()
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
                .insert_edge(i, i + 1, 0, &[("weight".to_string(), Value::Double(i as f64))], 100)
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.lkcs");
        let exported = table.export_snapshot(100).unwrap();
        let snapshot = ColdSnapshot::create(&exported, &path).unwrap();
        assert!(snapshot.vertex_capacity() >= 10);
    }
}

//! Persistence operations: serialization and deserialization to/from disk.
//!
//! Node-group sharded layout, version 4:
//! - `meta.bin`: header section only (label ids, label name, schema, next
//!   edge id), with the manifest commit tail appended so metadata and
//!   manifest share one atomic unit.
//! - `groups_manifest.bin`: address width plus existing out/in group id lists.
//! - `out_g{gid}.bin` / `in_g{gid}.bin`: header + one `CsrVariant` dump per
//!   existing group, written only for dirty groups; missing groups read as
//!   empty and never produce files.
//! - `ts_g{gid}.bin`: authoritative timestamps for the owning group's edges,
//!   falling with the same dirt as the group.
//! - `props_g{gid}.bin`: property rows for the owning group's edges,
//!   falling with the same dirt as the group.
//!
//! Old single-file layouts, version 1 metadata without a commit tail,
//! version 2 metadata with global timestamps, pre-version-4 manifests and
//! the legacy global `properties.bin` are rejected: loading requires the
//! manifest, the embedded tail must equal the manifest file, and trailing
//! bytes after any payload fail loudly instead of loading partially.

use super::super::{CsrBase, CsrVariant};
use super::mvcc::EdgeTimestamps;
use crate::edge::property_schema::PropertySchema;
use crate::edge::CsrWithProperties;
use crate::edge::EdgeSchema;
use crate::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use graphdb_core::types::EdgeId;
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) const EDGE_META_VERSION: u32 = 3;

/// Deserialized edge table metadata returned by [`load_metadata`].
/// Version 3 carries the header only; authoritative timestamps live in
/// per-group timestamp shards and are merged on load.
pub(crate) struct EdgeMetadata {
    pub label: u32,
    pub src_label: u32,
    pub dst_label: u32,
    pub label_name: String,
    pub is_open: bool,
    pub schema: EdgeSchema,
    pub next_edge_id: EdgeId,
}

/// Serialize edge table metadata to a buffer.
///
/// Layout version 3: header section only (label ids, label name, openness,
/// schema, next edge id). Timestamps are sharded per owner group in
/// `ts_g{gid}.bin` files falling with the same dirt as their topology group;
/// `meta.bin` never carries timestamps. The caller appends the manifest
/// commit tail after the header so metadata and manifest share one atomic
/// unit. Version 1 payloads (no tail) and version 2 payloads (global
/// timestamps) are rejected on load, never converted.
#[allow(clippy::too_many_arguments)]
pub fn flush_metadata(
    buf: &mut Vec<u8>,
    label: u32,
    src_label: u32,
    dst_label: u32,
    label_name: &str,
    is_open: bool,
    schema: &EdgeSchema,
    next_edge_id: EdgeId,
    _edge_timestamps: &HashMap<EdgeId, EdgeTimestamps>,
) -> StorageResult<()> {
    buf.extend_from_slice(&EDGE_META_VERSION.to_le_bytes());
    write_metadata_header(buf, label, src_label, dst_label, label_name, is_open, schema)?;
    write_metadata_next_edge_id(buf, next_edge_id);
    Ok(())
}

/// Header section: label identity, openness, schema and the edge-id counter.
/// Timestamps live in the following section and may split by group later.
#[allow(clippy::too_many_arguments)]
fn write_metadata_header(
    buf: &mut Vec<u8>,
    label: u32,
    src_label: u32,
    dst_label: u32,
    label_name: &str,
    is_open: bool,
    schema: &EdgeSchema,
) -> StorageResult<()> {
    buf.extend_from_slice(&label.to_le_bytes());
    buf.extend_from_slice(&src_label.to_le_bytes());
    buf.extend_from_slice(&dst_label.to_le_bytes());

    let label_name_bytes = label_name.as_bytes();
    buf.extend_from_slice(&(label_name_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(label_name_bytes);

    let is_open_flag: u8 = if is_open { 1 } else { 0 };
    buf.extend_from_slice(&is_open_flag.to_le_bytes());

    let schema_json =
        serde_json::to_string(schema).map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let schema_bytes = schema_json.as_bytes();
    buf.extend_from_slice(&(schema_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(schema_bytes);
    Ok(())
}

fn write_metadata_next_edge_id(buf: &mut Vec<u8>, next_edge_id: EdgeId) {
    buf.extend_from_slice(&next_edge_id.0.to_le_bytes());
}

/// Timestamp shard payload: authoritative stamps for one owner group.
/// Serialized as count plus `(edge_id, create_ts, delete_ts)` triples.
pub fn serialize_timestamp_shard(
    entries: &[(EdgeId, EdgeTimestamps)],
    section_id: u32,
    buf: &mut Vec<u8>,
) -> StorageResult<()> {
    write_header_to(buf, section_id)
        .map_err(|e| StorageError::io_error(format!("Failed to write ts shard header: {}", e)))?;
    buf.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for (edge_id, ts) in entries {
        buf.extend_from_slice(&edge_id.0.to_le_bytes());
        buf.extend_from_slice(&ts.create_ts.to_le_bytes());
        buf.extend_from_slice(&ts.delete_ts.to_le_bytes());
    }
    Ok(())
}

/// Load one timestamp shard payload, failing closed on section, length or
/// trailing-byte mismatches.
pub fn load_timestamp_shard(
    path: &Path,
    expected_section: u32,
) -> StorageResult<Vec<(EdgeId, EdgeTimestamps)>> {
    let (raw_data, _) = read_pages_from_file(path)?;
    let mut cursor = &raw_data[..];
    let mut header_buf = [0u8; HEADER_SIZE];
    cursor.read_exact(&mut header_buf)?;
    {
        let mut slice = &header_buf[..];
        let (_version, sid) = read_header(&mut slice)?;
        if sid != expected_section {
            return Err(StorageError::deserialize_error(format!(
                "unexpected section id in ts shard: expected {:#06x}, got {:#06x}",
                expected_section, sid
            )));
        }
    }
    let mut len_bytes = [0u8; 8];
    cursor.read_exact(&mut len_bytes)?;
    let len = u64::from_le_bytes(len_bytes) as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        let mut edge_id_bytes = [0u8; 8];
        cursor.read_exact(&mut edge_id_bytes)?;
        let mut create_bytes = [0u8; 8];
        cursor.read_exact(&mut create_bytes)?;
        let mut delete_bytes = [0u8; 8];
        cursor.read_exact(&mut delete_bytes)?;
        out.push((
            EdgeId(u64::from_le_bytes(edge_id_bytes)),
            EdgeTimestamps {
                create_ts: u64::from_le_bytes(create_bytes),
                delete_ts: u64::from_le_bytes(delete_bytes),
            },
        ));
    }
    if !cursor.is_empty() {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in ts shard".to_string(),
        ));
    }
    Ok(out)
}

/// Serialize one sharded property payload with an explicit section id.
/// The payload is a `CsrWithProperties` dump for the owning group's edges
/// only; column encoding and statistics travel with the shard and are
/// recomputed globally after the merge on load.
pub fn serialize_property_shard(
    properties: &CsrWithProperties,
    section_id: u32,
    buf: &mut Vec<u8>,
) -> StorageResult<()> {
    write_header_to(buf, section_id)
        .map_err(|e| StorageError::io_error(format!("Failed to write props shard header: {}", e)))?;
    let data = properties.dump();
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(&data);
    Ok(())
}

/// Serialize one sharded CSR to a buffer
pub fn serialize_csr(csr: &CsrVariant, section_id: u32, buf: &mut Vec<u8>) -> StorageResult<()> {
    write_header_to(buf, section_id)
        .map_err(|e| StorageError::io_error(format!("Failed to write CSR header: {}", e)))?;

    let data = csr.dump();
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(&data);

    Ok(())
}

pub fn serialize_csr_properties(
    properties: &CsrWithProperties,
    buf: &mut Vec<u8>,
) -> StorageResult<()> {
    write_header_to(buf, section::EDGE_PROPERTIES)
        .map_err(|e| StorageError::io_error(format!("Failed to write properties header: {}", e)))?;
    let data = properties.dump();
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(&data);
    Ok(())
}

/// Load metadata from file cursor
pub(crate) fn load_metadata(cursor: &mut &[u8]) -> StorageResult<EdgeMetadata> {
    let mut label_bytes = [0u8; 4];
    cursor.read_exact(&mut label_bytes)?;
    let label = u32::from_le_bytes(label_bytes);

    let mut src_label_bytes = [0u8; 4];
    cursor.read_exact(&mut src_label_bytes)?;
    let src_label = u32::from_le_bytes(src_label_bytes);

    let mut dst_label_bytes = [0u8; 4];
    cursor.read_exact(&mut dst_label_bytes)?;
    let dst_label = u32::from_le_bytes(dst_label_bytes);

    let mut label_name_len_bytes = [0u8; 4];
    cursor.read_exact(&mut label_name_len_bytes)?;
    let label_name_len = u32::from_le_bytes(label_name_len_bytes) as usize;

    let mut label_name_bytes = vec![0u8; label_name_len];
    cursor.read_exact(&mut label_name_bytes)?;
    let label_name = String::from_utf8(label_name_bytes)
        .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

    let mut is_open_bytes = [0u8; 1];
    cursor.read_exact(&mut is_open_bytes)?;
    let is_open = is_open_bytes[0] != 0;

    let mut schema_len_bytes = [0u8; 4];
    cursor.read_exact(&mut schema_len_bytes)?;
    let schema_len = u32::from_le_bytes(schema_len_bytes) as usize;
    let mut schema_bytes = vec![0u8; schema_len];
    cursor.read_exact(&mut schema_bytes)?;
    let schema_json = String::from_utf8(schema_bytes)
        .map_err(|e| StorageError::deserialize_error(e.to_string()))?;
    let schema = serde_json::from_str(&schema_json)
        .map_err(|e| StorageError::deserialize_error(e.to_string()))?;

    let mut next_edge_id_bytes = [0u8; 8];
    cursor.read_exact(&mut next_edge_id_bytes)?;
    let next_edge_id = EdgeId(u64::from_le_bytes(next_edge_id_bytes));

    Ok(EdgeMetadata {
        label,
        src_label,
        dst_label,
        label_name,
        is_open,
        schema,
        next_edge_id,
    })
}

/// Load one sharded CSR from file. Trailing bytes are rejected so old
/// multi-segment payloads fail loudly instead of loading partially.
/// `expected_section` must match the file section id; out/in files are not
/// interchangeable.
pub fn load_csr(
    path: &Path,
    csr: &mut CsrVariant,
    expected_section: u32,
) -> StorageResult<()> {
    let (raw_data, total_rows) = read_pages_from_file(path)?;
    let mut cursor = &raw_data[..];
    let mut header_buf = [0u8; HEADER_SIZE];
    cursor.read_exact(&mut header_buf)?;
    {
        let mut slice = &header_buf[..];
        let (_version, sid) = read_header(&mut slice)?;
        if sid != expected_section {
            return Err(StorageError::deserialize_error(format!(
                "unexpected section id in edge CSR: expected {:#06x}, got {:#06x}",
                expected_section, sid
            )));
        }
    }

    let mut len_bytes = [0u8; 8];
    cursor.read_exact(&mut len_bytes)?;
    let len = u64::from_le_bytes(len_bytes) as usize;

    let mut data = vec![0u8; len];
    cursor.read_exact(&mut data)?;

    csr.load(&data)?;

    if !cursor.is_empty() {
        return Err(StorageError::deserialize_error(
            "unexpected trailing data in edge CSR: old multi-segment format is not supported"
                .to_string(),
        ));
    }

    let loaded_edge_count = csr.edge_count() as u32;
    if total_rows > 0 && total_rows != loaded_edge_count {
        return Err(StorageError::deserialize_error(format!(
            "CSR total_rows mismatch: header={}, actual={}",
            total_rows, loaded_edge_count
        )));
    }

    Ok(())
}

pub fn load_csr_properties(
    path: &Path,
    property_schema: Vec<PropertySchema>,
) -> StorageResult<CsrWithProperties> {
    let (raw_data, total_rows) = read_pages_from_file(path)?;
    let mut cursor = &raw_data[..];
    let mut header_buf = [0u8; HEADER_SIZE];
    cursor.read_exact(&mut header_buf)?;
    {
        let mut slice = &header_buf[..];
        let (_version, sid) = read_header(&mut slice)?;
        if sid != section::EDGE_PROPERTIES {
            return Err(StorageError::deserialize_error(format!(
                "unexpected section id in edge properties: expected {:#06x}, got {:#06x}",
                section::EDGE_PROPERTIES,
                sid
            )));
        }
    }
    let mut len_bytes = [0u8; 8];
    cursor.read_exact(&mut len_bytes)?;
    let len = u64::from_le_bytes(len_bytes) as usize;
    let mut data = vec![0u8; len];
    cursor.read_exact(&mut data)?;
    // The live schema keys the payload columns by name, so values land in
    // the right columns; unknown payload columns are skipped.
    let mut properties = CsrWithProperties::new(property_schema);
    properties.load(&data)?;
    if total_rows > 0 && total_rows != properties.row_count() as u32 {
        return Err(StorageError::deserialize_error(format!(
            "csr properties total_rows mismatch: header={}, actual={}",
            total_rows,
            properties.row_count()
        )));
    }
    Ok(properties)
}

/// Write payload to file using page-level compression with shadow file atomic writes
pub fn write_pages_to_file(
    path: &Path,
    payload: &[u8],
    page_size: usize,
    level: i32,
    total_rows: u32,
) -> StorageResult<()> {
    let mut pages_buf = Vec::new();
    let mut writer = crate::compression::PageWriter::new(page_size, level);
    writer.write_all(&mut pages_buf, payload)?;

    let mut final_buf = Vec::new();
    let header = crate::compression::ColumnFileHeader {
        page_size,
        page_count: writer.page_count(),
        total_rows,
    };
    header.serialize(&mut final_buf)?;
    final_buf.extend_from_slice(&pages_buf);

    crate::compression::write_shadow_file(path, &final_buf)
}

/// Read pages from a page-compressed file.
/// Returns (decompressed_data, total_rows_from_header).
pub fn read_pages_from_file(path: &Path) -> StorageResult<(Vec<u8>, u32)> {
    let file = File::open(path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {}", path.display(), e)))?;
    let mut reader = std::io::BufReader::new(file);
    let header = crate::compression::ColumnFileHeader::deserialize(&mut reader)?;
    let total_rows = header.total_rows;
    let page_reader = crate::compression::PageReader::new(header.page_size);
    let data = page_reader.read_all(&mut reader, header.page_count)?;
    Ok((data, total_rows))
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::edge_table::core::EdgeStore;
    use graphdb_core::Value;

    fn create_edge_table() -> EdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::types::StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        EdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_flush_load_roundtrip() {
        let mut table = create_edge_table();

        let ts = 100u64;
        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], ts)
            .unwrap();
        table
            .insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.5))], ts)
            .unwrap();
        table
            .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(3.5))], ts)
            .unwrap();

        let temp_dir = tempfile::tempdir().expect("temporary edge table directory");

        table
            .flush(
                temp_dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded_table = create_edge_table();
        loaded_table
            .load(temp_dir.path())
            .expect("load should succeed");

        assert_eq!(loaded_table.out_edges(1, ts).len(), 2);
        assert_eq!(loaded_table.out_edges(2, ts).len(), 1);
        assert!(loaded_table.has_edge(1, 2, 0, ts));

        let deleted = loaded_table
            .delete_edge(1, 3, 0, ts + 1)
            .expect("delete_edge should work after load");
        assert!(deleted);
        assert!(!loaded_table.has_edge(1, 3, 0, ts + 1));
    }

    #[test]
    fn test_flush_load_preserves_deletions() {
        let mut table = create_edge_table();

        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.5))], 110)
            .unwrap();
        table.delete_edge(1, 2, 0, 200).unwrap();

        let temp_dir = tempfile::tempdir().expect("temporary edge table directory");

        table
            .flush(
                temp_dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded_table = create_edge_table();
        loaded_table
            .load(temp_dir.path())
            .expect("load should succeed");

        assert!(loaded_table.has_edge(1, 2, 0, 150));
        assert!(!loaded_table.has_edge(1, 2, 0, 250));
        assert!(loaded_table.has_edge(1, 3, 0, 250));
    }

    #[test]
    fn test_flush_load_preserves_edge_timestamps() {
        let mut table = create_edge_table();

        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
            .unwrap();
        table
            .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(3.0))], 300)
            .unwrap();

        // Verify edge_timestamps are populated before flush
        assert!(table.mvcc.edge_timestamps.len() >= 3);

        let temp_dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                temp_dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = create_edge_table();
        loaded.load(temp_dir.path()).expect("load should succeed");

        // edge_timestamps restored from metadata: MVCCManager is the
        // single visibility authority, so timestamps are asserted there
        // rather than through CSR row replicas.
        assert!(loaded.mvcc.edge_timestamps.len() >= 3);
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(0)),
            Some(100)
        );
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(1)),
            Some(200)
        );
        assert_eq!(
            loaded.mvcc.creation_ts_of(graphdb_core::types::EdgeId(2)),
            Some(300)
        );
    }
}

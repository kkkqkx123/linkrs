//! Persistence operations: serialization and deserialization to/from disk.
//!
//! Handles flush (write) and load (read) operations with support for
//! versioning, compression, and backward compatibility.

use super::super::{CsrBase, CsrVariant};
use super::segment::{CsrSegment, DeletionInfo};
use crate::core::types::{EdgeId, Timestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::edge::EdgeSchema;
use crate::storage::edge::PropertyTable;
use crate::storage::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

const EDGE_META_VERSION: u32 = 2;
const EDGE_ID_STORAGE_MODE_DIRECT: u8 = 0;
const EDGE_ID_STORAGE_MODE_SEPARATE: u8 = 1;

/// Serialize edge table metadata to a buffer
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
    tombstones: &HashMap<EdgeId, Timestamp>,
    min_active_snapshot_ts: Timestamp,
) -> StorageResult<()> {
    buf.extend_from_slice(&EDGE_META_VERSION.to_le_bytes());
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

    buf.extend_from_slice(&next_edge_id.0.to_le_bytes());
    buf.extend_from_slice(&(tombstones.len() as u64).to_le_bytes());
    for (edge_id, delete_ts) in tombstones {
        buf.extend_from_slice(&edge_id.0.to_le_bytes());
        buf.extend_from_slice(&delete_ts.to_le_bytes());
    }
    buf.extend_from_slice(&min_active_snapshot_ts.to_le_bytes());

    Ok(())
}

/// Serialize CSR and segments to a buffer
pub fn serialize_csr(
    csr: &CsrVariant,
    segments: &[CsrSegment],
    section_id: u32,
    buf: &mut Vec<u8>,
) -> StorageResult<()> {
    write_header_to(buf, section_id)
        .map_err(|e| StorageError::io_error(format!("Failed to write CSR header: {}", e)))?;

    let data = csr.dump();
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(&data);
    buf.extend_from_slice(&(segments.len() as u64).to_le_bytes());

    for segment in segments {
        buf.extend_from_slice(&segment.create_ts_min.to_le_bytes());
        buf.extend_from_slice(&segment.create_ts_max.to_le_bytes());
        let (delete_ts_min, delete_ts_max) = segment.deletion_range();
        buf.extend_from_slice(&delete_ts_min.to_le_bytes());
        buf.extend_from_slice(&delete_ts_max.to_le_bytes());
        let data = segment.csr.read().dump();
        buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
        buf.extend_from_slice(&data);

        if let Some(edge_ids) = &segment.edge_ids {
            buf.push(EDGE_ID_STORAGE_MODE_SEPARATE);
            buf.extend_from_slice(&(edge_ids.len() as u64).to_le_bytes());
            let mut edge_id_buffer = Vec::with_capacity(edge_ids.len() * 8);
            for edge_id in edge_ids {
                edge_id_buffer.extend_from_slice(&edge_id.to_le_bytes());
            }
            buf.extend_from_slice(&edge_id_buffer);
        } else {
            buf.push(EDGE_ID_STORAGE_MODE_DIRECT);
        }
    }

    Ok(())
}

/// Serialize properties to a buffer
pub fn serialize_properties(properties: &PropertyTable, buf: &mut Vec<u8>) -> StorageResult<()> {
    write_header_to(buf, section::EDGE_PROPERTIES)
        .map_err(|e| StorageError::io_error(format!("Failed to write properties header: {}", e)))?;

    let data = properties.dump();
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    buf.extend_from_slice(&data);

    Ok(())
}

/// Load metadata from file cursor
#[allow(clippy::type_complexity)]
pub fn load_metadata(
    cursor: &mut &[u8],
) -> StorageResult<(
    u32,
    u32,
    u32,
    String,
    bool,
    EdgeSchema,
    EdgeId,
    HashMap<EdgeId, Timestamp>,
    Timestamp,
)> {
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

    let mut tombstone_count_bytes = [0u8; 8];
    cursor.read_exact(&mut tombstone_count_bytes)?;
    let tombstone_count = u64::from_le_bytes(tombstone_count_bytes) as usize;
    let mut tombstones = HashMap::new();
    for _ in 0..tombstone_count {
        let mut edge_id_bytes = [0u8; 8];
        cursor.read_exact(&mut edge_id_bytes)?;
        let mut delete_ts_bytes = [0u8; 8];
        cursor.read_exact(&mut delete_ts_bytes)?;
        tombstones.insert(
            EdgeId(u64::from_le_bytes(edge_id_bytes)),
            u64::from_le_bytes(delete_ts_bytes),
        );
    }

    let mut min_snapshot_ts_bytes = [0u8; 8];
    cursor.read_exact(&mut min_snapshot_ts_bytes)?;
    let min_active_snapshot_ts = u64::from_le_bytes(min_snapshot_ts_bytes);

    Ok((
        label,
        src_label,
        dst_label,
        label_name,
        is_open,
        schema,
        next_edge_id,
        tombstones,
        min_active_snapshot_ts,
    ))
}

/// Load CSR and segments from file
pub fn load_csr(
    path: &Path,
    csr: &mut CsrVariant,
    segments: &mut Vec<CsrSegment>,
) -> StorageResult<()> {
    let (raw_data, total_rows) = read_pages_from_file(path)?;
    let mut cursor = &raw_data[..];
    let mut header_buf = [0u8; HEADER_SIZE];
    cursor.read_exact(&mut header_buf)?;
    {
        let mut slice = &header_buf[..];
        let (_version, sid) = read_header(&mut slice)?;
        if sid != section::EDGE_OUT_CSR && sid != section::EDGE_IN_CSR {
            return Err(StorageError::deserialize_error(format!(
                "unexpected section id in edge CSR: expected {:#06x} or {:#06x}, got {:#06x}",
                section::EDGE_OUT_CSR,
                section::EDGE_IN_CSR,
                sid
            )));
        }
    }

    let mut len_bytes = [0u8; 8];
    cursor.read_exact(&mut len_bytes)?;
    let len = u64::from_le_bytes(len_bytes) as usize;

    let mut data = vec![0u8; len];
    cursor.read_exact(&mut data)?;

    csr.load(&data)?;
    segments.clear();

    let mut segment_count_bytes = [0u8; 8];
    cursor.read_exact(&mut segment_count_bytes)?;
    let segment_count = u64::from_le_bytes(segment_count_bytes) as usize;

    for _ in 0..segment_count {
        let mut create_ts_min_bytes = [0u8; 8];
        cursor.read_exact(&mut create_ts_min_bytes)?;
        let create_ts_min = u64::from_le_bytes(create_ts_min_bytes);

        let mut create_ts_max_bytes = [0u8; 8];
        cursor.read_exact(&mut create_ts_max_bytes)?;
        let create_ts_max = u64::from_le_bytes(create_ts_max_bytes);

        let mut delete_ts_min_bytes = [0u8; 8];
        cursor.read_exact(&mut delete_ts_min_bytes)?;
        let delete_ts_min = u64::from_le_bytes(delete_ts_min_bytes);

        let mut delete_ts_max_bytes = [0u8; 8];
        cursor.read_exact(&mut delete_ts_max_bytes)?;
        let delete_ts_max = u64::from_le_bytes(delete_ts_max_bytes);

        let mut segment_len_bytes = [0u8; 8];
        cursor.read_exact(&mut segment_len_bytes)?;
        let segment_len = u64::from_le_bytes(segment_len_bytes) as usize;

        let mut segment_data = vec![0u8; segment_len];
        cursor.read_exact(&mut segment_data)?;

        let mut segment_csr = super::super::Csr::new();
        segment_csr.load(&segment_data)?;
        let deletion_info = DeletionInfo::new(delete_ts_min, delete_ts_max);
        let mut segment = CsrSegment::new(segment_csr, create_ts_min, create_ts_max, deletion_info);

        if !cursor.is_empty() {
            let mut mode_byte = [0u8; 1];
            cursor.read_exact(&mut mode_byte)?;
            match mode_byte[0] {
                EDGE_ID_STORAGE_MODE_DIRECT => {}
                EDGE_ID_STORAGE_MODE_SEPARATE => {
                    if cursor.len() < 8 {
                        return Err(StorageError::deserialize_error(
                            "truncated edge_id count in segment".to_string(),
                        ));
                    }
                    let mut edge_count_bytes = [0u8; 8];
                    cursor.read_exact(&mut edge_count_bytes)?;
                    let edge_count = u64::from_le_bytes(edge_count_bytes) as usize;

                    let csr_edge_count = segment.csr.read().edge_count() as usize;
                    if edge_count != csr_edge_count {
                        return Err(StorageError::deserialize_error(format!(
                            "edge_ids count mismatch: stored={}, csr={}",
                            edge_count, csr_edge_count
                        )));
                    }

                    if cursor.len() < edge_count * 8 {
                        return Err(StorageError::deserialize_error(format!(
                            "truncated edge_ids data: need {} bytes, have {}",
                            edge_count * 8,
                            cursor.len()
                        )));
                    }

                    let mut edge_ids = Vec::with_capacity(edge_count);
                    for _ in 0..edge_count {
                        let mut edge_id_bytes = [0u8; 8];
                        cursor.read_exact(&mut edge_id_bytes)?;
                        edge_ids.push(EdgeId(u64::from_le_bytes(edge_id_bytes)));
                    }
                    segment.edge_ids = Some(edge_ids);
                }
                _ => {
                    return Err(StorageError::deserialize_error(format!(
                        "unknown edge_id storage mode: {}",
                        mode_byte[0]
                    )));
                }
            }
        }

        segments.push(segment);
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

/// Load properties from file
pub fn load_properties(path: &Path) -> StorageResult<PropertyTable> {
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

    let mut properties = PropertyTable::new();
    properties.load(&data)?;

    if total_rows > 0 && total_rows != properties.row_count() as u32 {
        return Err(StorageError::deserialize_error(format!(
            "properties total_rows mismatch: header={}, actual={}",
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
    let mut writer = crate::storage::compression::PageWriter::new(page_size, level);
    writer.write_all(&mut pages_buf, payload)?;

    let mut final_buf = Vec::new();
    let header = crate::storage::compression::ColumnFileHeader {
        page_size,
        page_count: writer.page_count(),
        total_rows,
    };
    header.serialize(&mut final_buf)?;
    final_buf.extend_from_slice(&pages_buf);

    crate::storage::compression::write_shadow_file(path, &final_buf)
}

/// Read pages from a page-compressed file.
/// Returns (decompressed_data, total_rows_from_header).
pub fn read_pages_from_file(path: &Path) -> StorageResult<(Vec<u8>, u32)> {
    let file = File::open(path)
        .map_err(|e| StorageError::io_error(format!("Failed to open {}: {}", path.display(), e)))?;
    let mut reader = std::io::BufReader::new(file);
    let header = crate::storage::compression::ColumnFileHeader::deserialize(&mut reader)?;
    let total_rows = header.total_rows;
    let page_reader = crate::storage::compression::PageReader::new(header.page_size);
    let data = page_reader.read_all(&mut reader, header.page_count)?;
    Ok((data, total_rows))
}

#[cfg(test)]
mod tests {
    use super::super::super::*;
    use crate::core::Value;
    use crate::storage::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};

    fn create_edge_table() -> TimeTravelEdgeStore {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_flush_load_roundtrip() {
        let schema = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

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
                crate::storage::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema2 = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded_table =
            TimeTravelEdgeStore::with_config(schema2, EdgeTableConfig::default()).unwrap();
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
    fn test_flush_load_preserves_segments_and_tombstones() {
        let schema = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();

        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .unwrap();
        table
            .insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.5))], 110)
            .unwrap();
        table.freeze_csr_only(150);
        table.delete_edge(1, 2, 0, 200).unwrap();

        let temp_dir = tempfile::tempdir().expect("temporary edge table directory");

        table
            .flush(
                temp_dir.path(),
                crate::storage::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema2 = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![crate::storage::types::StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut loaded_table =
            TimeTravelEdgeStore::with_config(schema2, EdgeTableConfig::default()).unwrap();
        loaded_table
            .load(temp_dir.path())
            .expect("load should succeed");

        assert_eq!(loaded_table.out_segments.len(), 1);
        assert_eq!(loaded_table.in_segments.len(), 1);
        assert!(loaded_table.has_edge(1, 2, 0, 150));
        assert!(!loaded_table.has_edge(1, 2, 0, 250));
        assert!(loaded_table.has_edge(1, 3, 0, 250));
    }

    #[test]
    fn test_segment_size_estimation() {
        let mut table = create_edge_table();

        for i in 0..50u64 {
            table
                .insert_edge((i % 10) as u32, (100 + i) as u32, 0, &[], 1000 + i)
                .unwrap();
        }

        table.freeze_csr_only(1100);

        let total_bytes = table.segments_total_bytes();
        assert!(total_bytes > 0);
        assert!(total_bytes >= 50 * 20);
    }
}

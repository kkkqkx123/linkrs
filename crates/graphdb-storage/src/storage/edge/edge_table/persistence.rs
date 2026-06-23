//! Persistence operations: serialization and deserialization to/from disk.
//!
//! Handles flush (write) and load (read) operations with support for
//! versioning, compression, and backward compatibility.

use super::segment::{CsrSegment, DeletionInfo, SEPARATE_EDGE_ID_STORAGE_THRESHOLD};
use super::super::{CsrVariant, CsrBase, MutableCsrTrait};
use crate::core::types::{Timestamp, EdgeId};
use crate::core::{StorageError, StorageResult};
use crate::storage::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use crate::storage::edge::PropertyTable;
use crate::storage::edge::EdgeSchema;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

const EDGE_META_VERSION: u32 = 2;
const EDGE_ID_STORAGE_MODE_DIRECT: u8 = 0;
const EDGE_ID_STORAGE_MODE_SEPARATE: u8 = 1;

/// Write edge table metadata to file
pub fn flush_metadata(
    file: &mut File,
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
    file.write_all(&EDGE_META_VERSION.to_le_bytes())?;
    file.write_all(&label.to_le_bytes())?;
    file.write_all(&src_label.to_le_bytes())?;
    file.write_all(&dst_label.to_le_bytes())?;

    let label_name_bytes = label_name.as_bytes();
    file.write_all(&(label_name_bytes.len() as u32).to_le_bytes())?;
    file.write_all(label_name_bytes)?;

    let is_open_flag: u8 = if is_open { 1 } else { 0 };
    file.write_all(&is_open_flag.to_le_bytes())?;

    let schema_json = serde_json::to_string(schema)
        .map_err(|e| StorageError::serialize_error(e.to_string()))?;
    let schema_bytes = schema_json.as_bytes();
    file.write_all(&(schema_bytes.len() as u32).to_le_bytes())?;
    file.write_all(schema_bytes)?;

    file.write_all(&next_edge_id.0.to_le_bytes())?;
    file.write_all(&(tombstones.len() as u64).to_le_bytes())?;
    for (edge_id, delete_ts) in tombstones {
        file.write_all(&edge_id.0.to_le_bytes())?;
        file.write_all(&delete_ts.to_le_bytes())?;
    }
    file.write_all(&min_active_snapshot_ts.to_le_bytes())?;

    Ok(())
}

/// Flush CSR and segments to file
pub fn flush_csr(
    csr: &CsrVariant,
    segments: &[CsrSegment],
    path: &Path,
    section_id: u32,
) -> StorageResult<()> {
    let mut file = File::create(path)?;
    write_header_to(&mut file, section_id)
        .map_err(|e| StorageError::io_error(format!("Failed to write CSR header: {}", e)))?;

    let data = csr.dump();
    file.write_all(&(data.len() as u64).to_le_bytes())?;
    file.write_all(&data)?;
    file.write_all(&(segments.len() as u64).to_le_bytes())?;

    for segment in segments {
        file.write_all(&segment.create_ts_min.to_le_bytes())?;
        file.write_all(&segment.create_ts_max.to_le_bytes())?;
        let (delete_ts_min, delete_ts_max) = segment.deletion_range();
        file.write_all(&delete_ts_min.to_le_bytes())?;
        file.write_all(&delete_ts_max.to_le_bytes())?;
        let data = segment.csr.dump();
        file.write_all(&(data.len() as u64).to_le_bytes())?;
        file.write_all(&data)?;

        if let Some(edge_ids) = &segment.edge_ids {
            file.write_all(&[EDGE_ID_STORAGE_MODE_SEPARATE])?;
            file.write_all(&(edge_ids.len() as u64).to_le_bytes())?;
            let mut edge_id_buffer = Vec::with_capacity(edge_ids.len() * 8);
            for edge_id in edge_ids {
                edge_id_buffer.extend_from_slice(&edge_id.to_le_bytes());
            }
            file.write_all(&edge_id_buffer)?;
        } else {
            file.write_all(&[EDGE_ID_STORAGE_MODE_DIRECT])?;
        }
    }

    Ok(())
}

/// Flush properties to file
pub fn flush_properties(
    properties: &PropertyTable,
    path: &Path,
) -> StorageResult<()> {
    let mut file = File::create(path)?;
    write_header_to(&mut file, section::EDGE_PROPERTIES).map_err(|e| {
        StorageError::io_error(format!("Failed to write properties header: {}", e))
    })?;

    let data = properties.dump();
    file.write_all(&(data.len() as u64).to_le_bytes())?;
    file.write_all(&data)?;

    Ok(())
}

/// Load metadata from file cursor
pub fn load_metadata(
    cursor: &mut &[u8],
) -> StorageResult<(u32, u32, u32, String, bool, EdgeSchema, EdgeId, HashMap<EdgeId, Timestamp>, Timestamp)> {
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
        let mut delete_ts_bytes = [0u8; 4];
        cursor.read_exact(&mut delete_ts_bytes)?;
        tombstones.insert(
            EdgeId(u64::from_le_bytes(edge_id_bytes)),
            u32::from_le_bytes(delete_ts_bytes),
        );
    }

    let mut min_snapshot_ts_bytes = [0u8; 4];
    cursor.read_exact(&mut min_snapshot_ts_bytes)?;
    let min_active_snapshot_ts = u32::from_le_bytes(min_snapshot_ts_bytes);

    Ok((label, src_label, dst_label, label_name, is_open, schema, next_edge_id, tombstones, min_active_snapshot_ts))
}

/// Load CSR and segments from file
pub fn load_csr(
    path: &Path,
    csr: &mut CsrVariant,
    segments: &mut Vec<CsrSegment>,
) -> StorageResult<()> {
    let raw_data = crate::storage::compression::read_decompressed(path)?;
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
        let mut create_ts_min_bytes = [0u8; 4];
        cursor.read_exact(&mut create_ts_min_bytes)?;
        let create_ts_min = u32::from_le_bytes(create_ts_min_bytes);

        let mut create_ts_max_bytes = [0u8; 4];
        cursor.read_exact(&mut create_ts_max_bytes)?;
        let create_ts_max = u32::from_le_bytes(create_ts_max_bytes);

        let mut delete_ts_min_bytes = [0u8; 4];
        cursor.read_exact(&mut delete_ts_min_bytes)?;
        let delete_ts_min = u32::from_le_bytes(delete_ts_min_bytes);

        let mut delete_ts_max_bytes = [0u8; 4];
        cursor.read_exact(&mut delete_ts_max_bytes)?;
        let delete_ts_max = u32::from_le_bytes(delete_ts_max_bytes);

        let mut segment_len_bytes = [0u8; 8];
        cursor.read_exact(&mut segment_len_bytes)?;
        let segment_len = u64::from_le_bytes(segment_len_bytes) as usize;

        let mut segment_data = vec![0u8; segment_len];
        cursor.read_exact(&mut segment_data)?;

        let mut segment_csr = super::super::Csr::new();
        segment_csr.load(&segment_data)?;
        let deletion_info = DeletionInfo::new(delete_ts_min, delete_ts_max);
        let mut segment = CsrSegment::new(
            segment_csr,
            create_ts_min,
            create_ts_max,
            deletion_info,
        );

        if cursor.len() >= 1 {
            let mut mode_byte = [0u8; 1];
            cursor.read_exact(&mut mode_byte)?;
            match mode_byte[0] {
                EDGE_ID_STORAGE_MODE_DIRECT => {}
                EDGE_ID_STORAGE_MODE_SEPARATE => {
                    if cursor.len() < 8 {
                        return Err(StorageError::deserialize_error(
                            "truncated edge_id count in segment".to_string()
                        ));
                    }
                    let mut edge_count_bytes = [0u8; 8];
                    cursor.read_exact(&mut edge_count_bytes)?;
                    let edge_count = u64::from_le_bytes(edge_count_bytes) as usize;

                    let csr_edge_count = segment.csr.edge_count() as usize;
                    if edge_count != csr_edge_count {
                        return Err(StorageError::deserialize_error(
                            format!("edge_ids count mismatch: stored={}, csr={}", edge_count, csr_edge_count)
                        ));
                    }

                    if cursor.len() < edge_count * 8 {
                        return Err(StorageError::deserialize_error(
                            format!("truncated edge_ids data: need {} bytes, have {}",
                                edge_count * 8, cursor.len())
                        ));
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
                    return Err(StorageError::deserialize_error(
                        format!("unknown edge_id storage mode: {}", mode_byte[0])
                    ));
                }
            }
        }

        segments.push(segment);
    }

    Ok(())
}

/// Load properties from file
pub fn load_properties(
    path: &Path,
) -> StorageResult<PropertyTable> {
    let raw_data = crate::storage::compression::read_decompressed(path)?;
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

    Ok(properties)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use super::super::super::*;
    use crate::core::Value;

    fn create_edge_table_with_props() -> super::super::super::EdgeTable {
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
        super::super::super::EdgeTable::new(schema).unwrap()
    }

    fn create_edge_table() -> super::super::super::EdgeTable {
        let schema = super::super::super::EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
        schema_version: 1,
        };
        super::super::super::EdgeTable::new(schema).unwrap()
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
        let mut table = super::super::super::EdgeTable::new(schema).unwrap();

        let ts = 100u32;
        table.insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], ts).unwrap();
        table.insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.5))], ts).unwrap();
        table.insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(3.5))], ts).unwrap();

        let temp_dir = std::env::temp_dir().join("edge_table_test_flush_load");
        let _ = fs::remove_dir_all(&temp_dir);

        table.flush(&temp_dir, crate::storage::compression::CompressionType::Zstd { level: 3 }).expect("flush should succeed");

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
        let mut loaded_table = super::super::super::EdgeTable::new(schema2).unwrap();
        loaded_table.load(&temp_dir).expect("load should succeed");

        assert_eq!(loaded_table.out_edges(1, ts).len(), 2);
        assert_eq!(loaded_table.out_edges(2, ts).len(), 1);
        assert!(loaded_table.has_edge(1, 2, 0, ts));

        let deleted = loaded_table.delete_edge(1, 3, 0, ts + 1).expect("delete_edge should work after load");
        assert!(deleted);
        assert!(!loaded_table.has_edge(1, 3, 0, ts + 1));

        let _ = fs::remove_dir_all(&temp_dir);
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
        let mut table = super::super::super::EdgeTable::new(schema).unwrap();

        table.insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.5))], 100).unwrap();
        table.insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.5))], 110).unwrap();
        table.freeze_csr_only(150);
        table.delete_edge(1, 2, 0, 200).unwrap();

        let temp_dir = std::env::temp_dir().join("edge_table_test_segments_tombstones");
        let _ = fs::remove_dir_all(&temp_dir);

        table.flush(&temp_dir, crate::storage::compression::CompressionType::Zstd { level: 3 }).expect("flush should succeed");

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
        let mut loaded_table = super::super::super::EdgeTable::new(schema2).unwrap();
        loaded_table.load(&temp_dir).expect("load should succeed");

        assert_eq!(loaded_table.out_segments.len(), 1);
        assert_eq!(loaded_table.in_segments.len(), 1);
        assert!(loaded_table.has_edge(1, 2, 0, 150));
        assert!(!loaded_table.has_edge(1, 2, 0, 250));
        assert!(loaded_table.has_edge(1, 3, 0, 250));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_segment_size_estimation() {
        let mut table = create_edge_table();

        for i in 0..50 {
            table.insert_edge(i % 10, 100 + i, 0, &[], 1000 + i as u32).unwrap();
        }

        table.freeze_csr_only(1100);

        let total_bytes = table.segments_total_bytes();
        assert!(total_bytes > 0);
        assert!(total_bytes >= 50 * 20);
    }
}

//! Persistence operations: serialization and deserialization to/from disk.
//!
//! Handles flush (write) and load (read) operations with support for
//! versioning and compression.

use super::super::{CsrBase, CsrVariant};
use super::mvcc::EdgeTimestamps;
use super::segment::{CsrSegment, DeletionInfo};
use crate::edge::EdgeSchema;
use crate::edge::PropertyTable;
use crate::persistence::{read_header, section, write_header_to, HEADER_SIZE};
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub(crate) const EDGE_META_VERSION: u32 = 1;
const EDGE_ID_STORAGE_MODE_DIRECT: u8 = 0;
const EDGE_ID_STORAGE_MODE_SEPARATE: u8 = 1;

/// Deserialized edge table metadata returned by [`load_metadata`].
pub(crate) struct EdgeMetadata {
    pub label: u32,
    pub src_label: u32,
    pub dst_label: u32,
    pub label_name: String,
    pub is_open: bool,
    pub schema: EdgeSchema,
    pub next_edge_id: EdgeId,
    pub tombstones: HashMap<EdgeId, Timestamp>,
    pub min_snapshot_ts: Timestamp,
    pub edge_timestamps: HashMap<EdgeId, EdgeTimestamps>,
}

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
    edge_timestamps: &HashMap<EdgeId, EdgeTimestamps>,
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

    // v2: edge_timestamps
    buf.extend_from_slice(&(edge_timestamps.len() as u64).to_le_bytes());
    for (edge_id, ts) in edge_timestamps {
        buf.extend_from_slice(&edge_id.0.to_le_bytes());
        buf.extend_from_slice(&ts.create_ts.to_le_bytes());
        buf.extend_from_slice(&ts.delete_ts.to_le_bytes());
    }

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

        // Region metadata
        const REGION_MAGIC: u32 = 0x5245474E; // 'REGN'
        buf.extend_from_slice(&REGION_MAGIC.to_le_bytes());
        buf.extend_from_slice(&(segment.region_vertex_count as u64).to_le_bytes());
        buf.extend_from_slice(&(segment.regions.len() as u64).to_le_bytes());
        for r in &segment.regions {
            buf.extend_from_slice(&r.region_id.to_le_bytes());
            buf.extend_from_slice(&r.vertex_start.to_le_bytes());
            buf.extend_from_slice(&r.vertex_end.to_le_bytes());
            buf.extend_from_slice(&r.edge_count.to_le_bytes());
            buf.extend_from_slice(&r.deleted_count.to_le_bytes());
            let (del_min, del_max) = match r.deletion_info {
                super::segment::DeletionInfo::NoDeletes => (Timestamp::MAX, 0u64),
                super::segment::DeletionInfo::HasDeletes {
                    min_ts,
                    max_ts,
                    deleted_count: _,
                } => (min_ts, max_ts),
            };
            buf.extend_from_slice(&del_min.to_le_bytes());
            buf.extend_from_slice(&del_max.to_le_bytes());
            buf.extend_from_slice(&(r.estimated_bytes as u64).to_le_bytes());
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
pub fn load_metadata(cursor: &mut &[u8]) -> StorageResult<EdgeMetadata> {
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

    // edge_timestamps: creation + deletion timestamps per edge
    let edge_timestamps = if !cursor.is_empty() {
        let mut et_count_bytes = [0u8; 8];
        cursor.read_exact(&mut et_count_bytes)?;
        let et_count = u64::from_le_bytes(et_count_bytes) as usize;
        let mut edge_timestamps = HashMap::with_capacity(et_count);
        for _ in 0..et_count {
            let mut edge_id_bytes = [0u8; 8];
            cursor.read_exact(&mut edge_id_bytes)?;
            let mut create_ts_bytes = [0u8; 8];
            cursor.read_exact(&mut create_ts_bytes)?;
            let mut delete_ts_bytes = [0u8; 8];
            cursor.read_exact(&mut delete_ts_bytes)?;
            edge_timestamps.insert(
                EdgeId(u64::from_le_bytes(edge_id_bytes)),
                EdgeTimestamps {
                    create_ts: u64::from_le_bytes(create_ts_bytes),
                    delete_ts: u64::from_le_bytes(delete_ts_bytes),
                },
            );
        }
        edge_timestamps
    } else {
        HashMap::new()
    };

    Ok(EdgeMetadata {
        label,
        src_label,
        dst_label,
        label_name,
        is_open,
        schema,
        next_edge_id,
        tombstones,
        min_snapshot_ts: min_active_snapshot_ts,
        edge_timestamps,
    })
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

        // Region metadata
        {
            const REGION_MAGIC: u32 = 0x5245474E;
            let mut magic_bytes = [0u8; 4];
            cursor.read_exact(&mut magic_bytes)?;
            if u32::from_le_bytes(magic_bytes) != REGION_MAGIC {
                return Err(StorageError::deserialize_error(format!(
                    "invalid region magic: expected {:#010x}, got {:#010x}",
                    REGION_MAGIC,
                    u32::from_le_bytes(magic_bytes)
                )));
            }
            if cursor.len() < 16 {
                return Err(StorageError::deserialize_error(
                    "truncated region header".to_string(),
                ));
            }
            let mut rvc_bytes = [0u8; 8];
            cursor.read_exact(&mut rvc_bytes)?;
            let region_vertex_count = u64::from_le_bytes(rvc_bytes) as usize;
            let mut rlen_bytes = [0u8; 8];
            cursor.read_exact(&mut rlen_bytes)?;
            let region_len = u64::from_le_bytes(rlen_bytes) as usize;
            let mut regions = Vec::with_capacity(region_len);
            for _ in 0..region_len {
                if cursor.len() < 4 + 4 + 4 + 4 + 4 + 8 + 8 + 8 {
                    return Err(StorageError::deserialize_error(
                        "truncated region entry".to_string(),
                    ));
                }
                let mut rid_bytes = [0u8; 4];
                cursor.read_exact(&mut rid_bytes)?;
                let region_id = u32::from_le_bytes(rid_bytes);
                let mut vs_bytes = [0u8; 4];
                cursor.read_exact(&mut vs_bytes)?;
                let vertex_start = u32::from_le_bytes(vs_bytes);
                let mut ve_bytes = [0u8; 4];
                cursor.read_exact(&mut ve_bytes)?;
                let vertex_end = u32::from_le_bytes(ve_bytes);
                let mut ec_bytes = [0u8; 4];
                cursor.read_exact(&mut ec_bytes)?;
                let edge_count = u32::from_le_bytes(ec_bytes);
                let mut dc_bytes = [0u8; 4];
                cursor.read_exact(&mut dc_bytes)?;
                let deleted_count = u32::from_le_bytes(dc_bytes);
                let mut del_min_bytes = [0u8; 8];
                cursor.read_exact(&mut del_min_bytes)?;
                let del_min = u64::from_le_bytes(del_min_bytes);
                let mut del_max_bytes = [0u8; 8];
                cursor.read_exact(&mut del_max_bytes)?;
                let del_max = u64::from_le_bytes(del_max_bytes);
                let mut eb_bytes = [0u8; 8];
                cursor.read_exact(&mut eb_bytes)?;
                let estimated_bytes = u64::from_le_bytes(eb_bytes) as usize;
                let deletion_info =
                    super::segment::DeletionInfo::with_count(del_min, del_max, deleted_count);
                regions.push(super::segment::RegionMeta {
                    region_id,
                    vertex_start,
                    vertex_end,
                    edge_count,
                    deleted_count,
                    deletion_info,
                    estimated_bytes,
                });
            }
            segment.region_vertex_count = region_vertex_count;
            segment.regions = regions;
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
    use crate::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use graphdb_core::Value;

    fn create_edge_table() -> TimeTravelEdgeStore {
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
        TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap()
    }

    #[test]
    fn test_flush_load_roundtrip() {
        let schema = super::super::super::EdgeSchema {
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
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema2 = super::super::super::EdgeSchema {
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
            properties: vec![crate::types::StoragePropertyDef::new(
                "weight".to_string(),
                graphdb_core::types::DataType::Double,
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
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let schema2 = super::super::super::EdgeSchema {
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
                .insert_edge(
                    (i % 10) as u32,
                    (100 + i) as u32,
                    0,
                    &[("weight".to_string(), Value::Double(i as f64))],
                    1000 + i,
                )
                .unwrap();
        }

        table.freeze_csr_only(1100);

        let total_bytes = table.segments_total_bytes();
        assert!(total_bytes > 0);
        assert!(total_bytes >= 50 * 20);
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

        // edge_timestamps restored from metadata
        assert!(loaded.mvcc.edge_timestamps.len() >= 3);

        // CSR create_ts_cache rebuilt from edge_timestamps
        use crate::edge::csr_trait::MutableCsrTrait;
        assert_eq!(
            loaded.out_csr.create_ts_of(graphdb_core::types::EdgeId(0)),
            Some(100)
        );
        assert_eq!(
            loaded.out_csr.create_ts_of(graphdb_core::types::EdgeId(1)),
            Some(200)
        );
        assert_eq!(
            loaded.out_csr.create_ts_of(graphdb_core::types::EdgeId(2)),
            Some(300)
        );
    }

    #[test]
    fn test_flush_load_create_ts_used_by_freeze() {
        let mut table = create_edge_table();

        // Insert edges at different timestamps
        table
            .insert_edge(1, 2, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .unwrap();
        table
            .insert_edge(1, 3, 0, &[("weight".to_string(), Value::Double(2.0))], 200)
            .unwrap();

        let temp_dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                temp_dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush should succeed");

        let mut loaded = create_edge_table();
        loaded.load(temp_dir.path()).expect("load should succeed");

        // Freeze at ts=150: only edge created at 100 should be included in segment
        loaded.freeze_csr_only(150);

        // After freeze, the segment should have correct create_ts_min
        assert!(!loaded.out_segments.is_empty());
        let seg = &loaded.out_segments[0];
        assert_eq!(seg.create_ts_min, 100);
    }
}

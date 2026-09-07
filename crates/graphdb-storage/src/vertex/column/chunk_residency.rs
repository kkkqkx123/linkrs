//! Chunk eviction and reload infrastructure.
//!
//! `ChunkResidency` tracks whether a chunk's data is in memory (Resident) or
//! has been spilled to disk (Evicted). Evicted chunks can be reloaded on demand
//! when a read or write targets their row range.

use std::io::Read;
use std::path::PathBuf;

use graphdb_core::{DataType, StorageError, StorageResult};

use crate::encoding::ColumnEncoding;
use super::chunk::ColumnChunk;

// ---------------------------------------------------------------------------
// ChunkResidency
// ---------------------------------------------------------------------------

/// Memory residency state of a [`ColumnChunk`].
#[derive(Debug, Clone)]
pub enum ChunkResidency {
    /// Data is in memory and accessible.
    Resident,
    /// Data has been spilled to disk; `spill_path` points to the sidecar file
    /// and `spill_size` records the serialized byte count for memory accounting.
    Evicted {
        spill_path: PathBuf,
        spill_size: u64,
    },
}

impl Default for ChunkResidency {
    fn default() -> Self {
        Self::Resident
    }
}

// ---------------------------------------------------------------------------
// Spill format (binary sidecar for one evicted chunk)
// ---------------------------------------------------------------------------

/// Binary layout version for the chunk spill format.
const SPILL_FORMAT_VERSION: u8 = 1;

/// Serialize a resident chunk to a byte buffer suitable for disk spill.
///
/// Format:
/// ```text
/// [u8 version]
/// [u32 row_offset] [u32 row_count] [u32 element_size] [u8 nullable]
/// [u8 data_type_tag]
/// [u8 enc_type] [u32 enc_meta_len] [enc_meta bytes]
/// [u32 data_len] [data bytes]
/// [u32 offsets_len] [offsets bytes]
/// [u32 bitmap_len] [bitmap bytes]
/// [u8 has_overlay] [overlay bytes (postcard)]
/// [u32 meta_len] [encoding_meta bytes]
/// ```
pub fn spill_chunk(chunk: &ColumnChunk, data_type: &DataType) -> StorageResult<Vec<u8>> {
    let mut buf = Vec::with_capacity(256);

    buf.push(SPILL_FORMAT_VERSION);

    buf.extend_from_slice(&(chunk.row_offset as u32).to_le_bytes());
    buf.extend_from_slice(&(chunk.row_count as u32).to_le_bytes());
    buf.extend_from_slice(&(chunk.element_size as u32).to_le_bytes());
    buf.push(chunk.null_bitmap.is_some() as u8);

    // Data type tag (for encoding deserialization: RleBool vs RleInt)
    buf.push(data_type_to_tag(data_type));

    // Encoding body — serialize_meta includes a leading tag byte that
    // duplicates the enc_type we write separately, so strip it.
    buf.push(chunk.encoding.encoding_type().to_u8());
    let mut enc_buf = Vec::new();
    chunk.encoding.serialize_meta(&mut enc_buf)?;
    // enc_buf[0] is the tag byte written by serialize_meta; skip it.
    let enc_body = if enc_buf.len() > 1 { &enc_buf[1..] } else { &[] };
    buf.extend_from_slice(&(enc_body.len() as u32).to_le_bytes());
    buf.extend_from_slice(enc_body);

    // Raw data
    buf.extend_from_slice(&(chunk.data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&chunk.data);

    // Offsets (u64 each)
    buf.extend_from_slice(&(chunk.offsets.len() as u32).to_le_bytes());
    for off in &chunk.offsets {
        buf.extend_from_slice(&off.to_le_bytes());
    }

    // Null bitmap
    match &chunk.null_bitmap {
        Some(bm) => {
            let raw = bm.as_raw_slice();
            buf.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            buf.extend_from_slice(raw);
            buf.extend_from_slice(&(bm.len() as u32).to_le_bytes());
        }
        None => {
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
    }

    // Overlay — length-prefixed so postcard doesn't consume trailing bytes.
    let overlay_entries: Vec<(u32, Option<graphdb_core::Value>)> =
        chunk.overlay.iter().map(|(k, v)| (*k, v.clone())).collect();
    let has_overlay = !overlay_entries.is_empty();
    buf.push(has_overlay as u8);
    if has_overlay {
        let overlay_bytes = postcard::to_allocvec(&overlay_entries)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        buf.extend_from_slice(&(overlay_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(&overlay_bytes);
    }

    // Encoding metadata
    let mut meta_buf = Vec::new();
    chunk.encoding_meta.serialize(&mut meta_buf)?;
    buf.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
    buf.extend_from_slice(&meta_buf);

    Ok(buf)
}

/// Deserialize a chunk spill buffer back into a [`ColumnChunk`].
///
/// The data type is read from the spill format itself (stored as a tag).
pub fn reload_chunk(bytes: &[u8]) -> StorageResult<ColumnChunk> {
    let mut cursor = &bytes[..];

    let mut ver = [0u8; 1];
    cursor.read_exact(&mut ver)?;
    if ver[0] != SPILL_FORMAT_VERSION {
        return Err(StorageError::deserialize_error(format!(
            "unsupported chunk spill version {}",
            ver[0]
        )));
    }

    let mut u32b = [0u8; 4];
    cursor.read_exact(&mut u32b)?;
    let row_offset = u32::from_le_bytes(u32b) as usize;
    cursor.read_exact(&mut u32b)?;
    let row_count = u32::from_le_bytes(u32b) as usize;
    cursor.read_exact(&mut u32b)?;
    let element_size = u32::from_le_bytes(u32b) as usize;
    let mut flag = [0u8; 1];
    cursor.read_exact(&mut flag)?;
    let nullable = flag[0] != 0;

    // Data type tag
    let mut dt_tag = [0u8; 1];
    cursor.read_exact(&mut dt_tag)?;
    let chunk_data_type = tag_to_data_type(dt_tag[0]);

    // Encoding
    cursor.read_exact(&mut flag)?;
    let enc_type = crate::encoding::EncodingType::from_u8(flag[0]);
    cursor.read_exact(&mut u32b)?;
    let enc_meta_len = u32::from_le_bytes(u32b) as usize;
    let enc_meta_bytes = take_bytes(&mut cursor, enc_meta_len as u32, "encoding meta")?;
    let encoding = if enc_type == crate::encoding::EncodingType::None {
        ColumnEncoding::None
    } else {
        decode_encoding_for_spill(enc_type, &enc_meta_bytes, &chunk_data_type)?
    };

    // Data
    cursor.read_exact(&mut u32b)?;
    let data_len = u32::from_le_bytes(u32b) as usize;
    let data = take_bytes(&mut cursor, data_len as u32, "chunk data")?;

    // Offsets
    cursor.read_exact(&mut u32b)?;
    let offsets_count = u32::from_le_bytes(u32b) as usize;
    let mut offsets = Vec::with_capacity(offsets_count);
    for _ in 0..offsets_count {
        let mut u64b = [0u8; 8];
        cursor.read_exact(&mut u64b)?;
        offsets.push(u64::from_le_bytes(u64b));
    }

    // Null bitmap
    cursor.read_exact(&mut u32b)?;
    let bitmap_bytes_len = u32::from_le_bytes(u32b) as usize;
    let bitmap_raw = if bitmap_bytes_len > 0 {
        Some(take_bytes(&mut cursor, bitmap_bytes_len as u32, "null bitmap")?)
    } else {
        None
    };
    cursor.read_exact(&mut u32b)?;
    let bitmap_bit_len = u32::from_le_bytes(u32b) as usize;
    let null_bitmap = if let Some(raw) = bitmap_raw {
        let mut bv = bitvec::vec::BitVec::from_vec(raw);
        bv.resize(bitmap_bit_len, false);
        Some(bv)
    } else if nullable {
        // Nullable column with zero nulls: preserve nullable semantics.
        Some(bitvec::vec::BitVec::new())
    } else {
        None
    };

    // Overlay — length-prefixed.
    let mut has_overlay = [0u8; 1];
    cursor.read_exact(&mut has_overlay)?;
    let overlay_entries: Vec<(u32, Option<graphdb_core::Value>)> =
        if has_overlay[0] != 0 {
            cursor.read_exact(&mut u32b)?;
            let ol_len = u32::from_le_bytes(u32b) as usize;
            let ol_bytes = take_bytes(&mut cursor, ol_len as u32, "overlay")?;
            postcard::from_bytes(&ol_bytes)
                .map_err(|e| StorageError::deserialize_error(e.to_string()))?
        } else {
            Vec::new()
        };

    // Encoding metadata
    cursor.read_exact(&mut u32b)?;
    let meta_len = u32::from_le_bytes(u32b) as usize;
    let encoding_meta = if meta_len > 0 {
        let meta_bytes = take_bytes(&mut cursor, meta_len as u32, "encoding meta")?;
        crate::encoding::ChunkEncodingMeta::deserialize(&mut &meta_bytes[..])?
    } else {
        crate::encoding::ChunkEncodingMeta::default()
    };

    let mut chunk = ColumnChunk {
        row_offset,
        row_count,
        data,
        offsets,
        null_bitmap,
        encoding,
        dirty_tracker: crate::persistence::dirty_page::DirtyPageTracker::new(0),
        version_chains: None,
        visibility: super::mvcc::RowVisibility::new(),
        element_size,
        overlay: super::chunk_encoding::UpdateOverlay::new(super::chunk_encoding::DEFAULT_OVERLAY_CAPACITY),
        encoding_meta,
        updates_since_encode: 0,
        residency: ChunkResidency::Resident,
        spill_path: None,
        spill_size: 0,
    };
    for (local, v) in overlay_entries {
        chunk.overlay.put(local, v);
    }
    chunk.updates_since_encode = chunk.overlay.len() as u64;

    Ok(chunk)
}

// ---------------------------------------------------------------------------
// Data type tag helpers (compact wire format for spill files)
// ---------------------------------------------------------------------------

fn data_type_to_tag(dt: &DataType) -> u8 {
    match dt {
        DataType::Bool => 1,
        DataType::SmallInt => 2,
        DataType::Int => 3,
        DataType::BigInt => 4,
        DataType::Float => 5,
        DataType::Double => 6,
        DataType::String => 7,
        DataType::Date => 8,
        DataType::Time => 9,
        DataType::DateTime => 10,
        DataType::Uuid => 11,
        _ => 0, // everything else falls back to Int-safe decoding
    }
}

fn tag_to_data_type(tag: u8) -> DataType {
    match tag {
        1 => DataType::Bool,
        2 => DataType::SmallInt,
        3 => DataType::Int,
        4 => DataType::BigInt,
        5 => DataType::Float,
        6 => DataType::Double,
        7 => DataType::String,
        8 => DataType::Date,
        9 => DataType::Time,
        10 => DataType::DateTime,
        11 => DataType::Uuid,
        _ => DataType::Int,
    }
}

// ---------------------------------------------------------------------------
// Encoding deserialization (duplicated from persistence to avoid circular dep)
// ---------------------------------------------------------------------------

fn decode_encoding_for_spill(
    encoding_type: crate::encoding::EncodingType,
    meta_bytes: &[u8],
    data_type: &DataType,
) -> StorageResult<ColumnEncoding> {
    use crate::encoding::{
        AlpColumn, BitPackedIntColumn, ConstantColumn, DictionaryColumn,
        FsstColumn, RleBoolColumn, RleIntColumn,
    };
    let mut cursor = &meta_bytes[..];
    match encoding_type {
        crate::encoding::EncodingType::Fsst => Ok(ColumnEncoding::Fsst(
            FsstColumn::deserialize_meta(&mut cursor)?,
        )),
        crate::encoding::EncodingType::Dictionary => Ok(ColumnEncoding::Dictionary(
            DictionaryColumn::deserialize_meta(&mut cursor)?,
        )),
        crate::encoding::EncodingType::Rle => {
            if *data_type == DataType::Bool {
                Ok(ColumnEncoding::RleBool(RleBoolColumn::deserialize_meta(
                    &mut cursor,
                )?))
            } else {
                Ok(ColumnEncoding::RleInt(RleIntColumn::deserialize_meta(
                    &mut cursor,
                )?))
            }
        }
        crate::encoding::EncodingType::BitPacking => Ok(ColumnEncoding::BitPacked(
            BitPackedIntColumn::deserialize_meta(&mut cursor)?,
        )),
        crate::encoding::EncodingType::Alp => Ok(ColumnEncoding::Alp(
            AlpColumn::deserialize_meta(&mut cursor)?,
        )),
        crate::encoding::EncodingType::Constant => Ok(ColumnEncoding::Constant(
            ConstantColumn::deserialize_meta(&mut cursor)?,
        )),
        crate::encoding::EncodingType::None => Ok(ColumnEncoding::None),
    }
}

fn take_bytes(cursor: &mut &[u8], len: u32, field: &str) -> StorageResult<Vec<u8>> {
    let len = len as usize;
    if len > cursor.len() {
        return Err(StorageError::deserialize_error(format!(
            "{} length {} exceeds remaining input {}",
            field,
            len,
            cursor.len()
        )));
    }
    let (value, remaining) = cursor.split_at(len);
    *cursor = remaining;
    Ok(value.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::EncodingType;

    #[test]
    fn spill_reload_raw_chunk() {
        let mut chunk = ColumnChunk::new(0, 4, 4, true);
        chunk.data = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

        let buf = spill_chunk(&chunk, &DataType::Int).unwrap();
        let reloaded = reload_chunk(&buf).unwrap();

        assert_eq!(reloaded.row_offset, 0);
        assert_eq!(reloaded.row_count, 4);
        assert_eq!(reloaded.element_size, 4);
        assert_eq!(reloaded.data, chunk.data);
        assert_eq!(reloaded.encoding_type(), EncodingType::None);
    }

    #[test]
    fn spill_reload_variable_chunk() {
        let mut chunk = ColumnChunk::new_variable(100, 3, true);
        chunk.offsets = vec![0, 5, 10];
        chunk.data = vec![
            3, 0, 0, 0, 0, b'h', b'e', b'l', b'l', b'o', 5, 0, 0, 0, 0, b'w', b'o', b'r',
            b'l', b'd',
        ];

        let buf = spill_chunk(&chunk, &DataType::String).unwrap();
        let reloaded = reload_chunk(&buf).unwrap();

        assert_eq!(reloaded.row_offset, 100);
        assert_eq!(reloaded.row_count, 3);
        assert_eq!(reloaded.offsets, chunk.offsets);
        assert_eq!(reloaded.data, chunk.data);
    }

    #[test]
    fn spill_reload_with_overlay() {
        let mut chunk = ColumnChunk::new(0, 4, 4, false);
        chunk.data = vec![0; 16];
        chunk
            .overlay
            .put(1, Some(graphdb_core::Value::Int(42)));
        chunk.overlay.put(3, None);

        let buf = spill_chunk(&chunk, &DataType::Int).unwrap();
        let reloaded = reload_chunk(&buf).unwrap();

        assert_eq!(
            reloaded.overlay.get(1),
            Some(Some(graphdb_core::Value::Int(42)))
        );
        assert_eq!(reloaded.overlay.get(3), Some(None));
        assert_eq!(reloaded.overlay.len(), 2);
    }

    #[test]
    fn spill_reload_roundtrip_encoding_meta() {
        let mut chunk = ColumnChunk::new(0, 8, 4, false);
        chunk.data = vec![0; 32];
        chunk.encoding_meta = crate::encoding::ChunkEncodingMeta {
            scheme: EncodingType::BitPacking,
            num_values: 8,
            all_null: false,
            min: Some(graphdb_core::Value::Int(0)),
            max: Some(graphdb_core::Value::Int(7)),
            bit_width: Some(3),
            alp_exceptions: None,
            compressed_size: 32,
            raw_size: 32,
        };

        let buf = spill_chunk(&chunk, &DataType::Int).unwrap();
        let reloaded = reload_chunk(&buf).unwrap();

        assert_eq!(reloaded.encoding_meta.scheme, EncodingType::BitPacking);
        assert_eq!(reloaded.encoding_meta.num_values, 8);
        assert_eq!(
            reloaded.encoding_meta.min,
            Some(graphdb_core::Value::Int(0))
        );
        assert_eq!(
            reloaded.encoding_meta.max,
            Some(graphdb_core::Value::Int(7))
        );
        assert_eq!(reloaded.encoding_meta.bit_width, Some(3));
    }
}

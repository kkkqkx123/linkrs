//! Persistence encoding framework
//!
//! Provides standardized file headers with magic bytes and versioning
//! for all persistence files in the storage layer.

use crate::core::error::StorageError;
use crate::core::StorageResult;

/// Magic bytes identifying GraphDB persistence files
pub const PERSISTENCE_MAGIC: [u8; 4] = *b"GRDB";

/// Current persistence format version
pub const CURRENT_VERSION: u32 = 1;

/// Header size in bytes: magic(4) + version(4) + section_id(4) = 12
pub const HEADER_SIZE: usize = 12;

/// Section IDs for different file types
pub mod section {
    pub const VERTEX_META: u32 = 0x0101;
    pub const VERTEX_ID_INDEXER: u32 = 0x0102;
    pub const VERTEX_COLUMNS: u32 = 0x0103;
    pub const VERTEX_TIMESTAMPS: u32 = 0x0104;

    pub const EDGE_META: u32 = 0x0201;
    pub const EDGE_OUT_CSR: u32 = 0x0202;
    pub const EDGE_IN_CSR: u32 = 0x0203;
    pub const EDGE_PROPERTIES: u32 = 0x0204;

    pub const PROPERTY_TABLE: u32 = 0x0301;
}

/// Write a persistence header (magic + version + section_id) into a buffer
pub fn write_header(buf: &mut Vec<u8>, section_id: u32) {
    buf.extend_from_slice(&PERSISTENCE_MAGIC);
    buf.extend_from_slice(&CURRENT_VERSION.to_le_bytes());
    buf.extend_from_slice(&section_id.to_le_bytes());
}

/// Validate and consume a persistence header from a byte slice.
/// Returns `(version, section_id)` on success.
pub fn read_header(data: &mut &[u8]) -> StorageResult<(u32, u32)> {
    if data.len() < HEADER_SIZE {
        return Err(StorageError::deserialize_error(format!(
            "data too short for header: {} bytes < {}",
            data.len(),
            HEADER_SIZE
        )));
    }

    let magic = &data[..4];
    if magic != PERSISTENCE_MAGIC {
        return Err(StorageError::deserialize_error(format!(
            "invalid magic bytes: {magic:02x?}"
        )));
    }
    *data = &data[4..];

    let version_bytes: [u8; 4] = data[..4]
        .try_into()
        .map_err(|_| StorageError::deserialize_error("failed to read version"))?;
    let version = u32::from_le_bytes(version_bytes);
    *data = &data[4..];

    let section_bytes: [u8; 4] = data[..4]
        .try_into()
        .map_err(|_| StorageError::deserialize_error("failed to read section_id"))?;
    let section_id = u32::from_le_bytes(section_bytes);
    *data = &data[4..];

    Ok((version, section_id))
}

/// Helper to write a header directly to a `std::io::Write` implementor
pub fn write_header_to<W: std::io::Write>(writer: &mut W, section_id: u32) -> std::io::Result<()> {
    writer.write_all(&PERSISTENCE_MAGIC)?;
    writer.write_all(&CURRENT_VERSION.to_le_bytes())?;
    writer.write_all(&section_id.to_le_bytes())?;
    Ok(())
}

/// Read a u64 from data at offset (little-endian), advancing offset
pub fn read_u64_le(data: &[u8], offset: &mut usize) -> StorageResult<u64> {
    let end = *offset + 8;
    if end > data.len() {
        return Err(StorageError::deserialize_error(format!(
            "unexpected end of data: needed {} bytes, have {} at offset {}",
            8,
            data.len(),
            *offset
        )));
    }
    let bytes: [u8; 8] = data[*offset..end]
        .try_into()
        .map_err(|_| StorageError::deserialize_error("failed to read u64"))?;
    *offset = end;
    Ok(u64::from_le_bytes(bytes))
}

/// Read a u32 from data at offset (little-endian), advancing offset
pub fn read_u32_le(data: &[u8], offset: &mut usize) -> StorageResult<u32> {
    let end = *offset + 4;
    if end > data.len() {
        return Err(StorageError::deserialize_error(format!(
            "unexpected end of data: needed {} bytes, have {} at offset {}",
            4,
            data.len(),
            *offset
        )));
    }
    let bytes: [u8; 4] = data[*offset..end]
        .try_into()
        .map_err(|_| StorageError::deserialize_error("failed to read u32"))?;
    *offset = end;
    Ok(u32::from_le_bytes(bytes))
}

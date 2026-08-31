//! Persistence encoding framework
//!
//! Provides standardized file headers with magic bytes and versioning
//! for all persistence files in the storage layer.

use graphdb_core::error::StorageError;
use graphdb_core::StorageResult;
use std::path::Path;

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

    #[allow(dead_code)]
    pub const PROPERTY_TABLE: u32 = 0x0301;
}

/// Write a persistence header (magic + version + section_id) into a buffer
#[allow(dead_code)]
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

/// Magic bytes for versioned payload wrapper (LNKF = LinkRs File)
pub const VERSIONED_PAYLOAD_MAGIC: [u8; 4] = *b"LNKF";

/// Write a versioned payload wrapper: [LNKF][version:u32][payload]
pub fn write_versioned_payload(buf: &mut Vec<u8>, version: u32, payload: &[u8]) {
    buf.extend_from_slice(&VERSIONED_PAYLOAD_MAGIC);
    buf.extend_from_slice(&version.to_le_bytes());
    buf.extend_from_slice(payload);
}

/// Atomically write `bytes` to `path`: write to a temporary sibling file,
/// fsync it, rename over the target, then fsync the parent directory so the
/// rename is durable. On any error the target path is left untouched.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> StorageResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| StorageError::db_error(format!("Path {} has no parent", path.display())))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Read and validate a versioned payload from a reader.
/// Returns the version and remaining payload bytes on success.
pub fn read_versioned_payload<R: std::io::Read>(
    reader: &mut R,
    file_name: &str,
) -> StorageResult<(u32, Vec<u8>)> {
    let mut magic = [0u8; 4];
    reader.read_exact(&mut magic).map_err(|e| {
        StorageError::deserialize_error(format!("{file_name}: failed to read magic: {e}"))
    })?;
    if magic != VERSIONED_PAYLOAD_MAGIC {
        return Err(StorageError::deserialize_error(format!(
            "{file_name}: invalid magic bytes {magic:02x?}, expected LNKF"
        )));
    }
    let mut version_buf = [0u8; 4];
    reader.read_exact(&mut version_buf).map_err(|e| {
        StorageError::deserialize_error(format!("{file_name}: failed to read version: {e}"))
    })?;
    let version = u32::from_le_bytes(version_buf);
    if version < graphdb_core::types::StorageVersion::MIN_SUPPORTED as u32 {
        return Err(StorageError::unsupported_version(
            version,
            graphdb_core::types::StorageVersion::CURRENT as u32,
        ));
    }
    let mut payload = Vec::new();
    reader.read_to_end(&mut payload).map_err(|e| {
        StorageError::deserialize_error(format!("{file_name}: failed to read payload: {e}"))
    })?;
    Ok((version, payload))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_header_valid() {
        let mut buf = Vec::new();
        write_header(&mut buf, section::VERTEX_META);
        let mut slice = &buf[..];
        let (version, section_id) = read_header(&mut slice).unwrap();
        assert_eq!(version, CURRENT_VERSION);
        assert_eq!(section_id, section::VERTEX_META);
    }

    #[test]
    fn test_read_header_rejects_bad_magic() {
        let mut buf = b"BADM".to_vec();
        buf.extend_from_slice(&CURRENT_VERSION.to_le_bytes());
        buf.extend_from_slice(&section::VERTEX_META.to_le_bytes());
        let mut slice = &buf[..];
        assert!(read_header(&mut slice).is_err());
    }

    #[test]
    fn test_read_header_rejects_too_short() {
        let buf = b"GR";
        let mut slice = &buf[..];
        assert!(read_header(&mut slice).is_err());
    }

    #[test]
    fn test_read_u32_le_rejects_truncated() {
        let data = [0x01, 0x02];
        let mut offset = 0;
        assert!(read_u32_le(&data, &mut offset).is_err());
    }

    #[test]
    fn test_read_u64_le_rejects_truncated() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let mut offset = 0;
        assert!(read_u64_le(&data, &mut offset).is_err());
    }

    #[test]
    fn test_write_versioned_payload_roundtrip() {
        let payload = b"hello world";
        let mut buf = Vec::new();
        write_versioned_payload(&mut buf, 1, payload);
        let mut reader = std::io::Cursor::new(buf);
        let (version, result) = read_versioned_payload(&mut reader, "test").unwrap();
        assert_eq!(version, 1);
        assert_eq!(result, payload);
    }

    #[test]
    fn test_read_versioned_payload_rejects_bad_magic() {
        let mut buf = b"BADM".to_vec();
        buf.extend_from_slice(&1u32.to_le_bytes());
        let mut reader = std::io::Cursor::new(buf);
        assert!(read_versioned_payload(&mut reader, "test").is_err());
    }

    #[test]
    fn test_read_versioned_payload_rejects_unsupported_version() {
        let mut buf = Vec::new();
        write_versioned_payload(&mut buf, 0, b"data");
        let mut reader = std::io::Cursor::new(buf);
        let err = read_versioned_payload(&mut reader, "test").unwrap_err();
        assert_eq!(
            err.kind(),
            graphdb_core::error::storage::StorageErrorKind::UnsupportedVersion
        );
    }
}

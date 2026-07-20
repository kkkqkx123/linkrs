//! Compression type definition and compression/decompression helpers for storage layer.
//!
//! This module provides the `CompressionType` enum for configuring
//! compression in flush operations, along with `compress_payload` and
//! `decompress_payload` helpers used by the table flush/load pipeline.
//!
//! Every persisted file uses the compression marker format:
//! - Marker 0x00: raw data follows
//! - Marker 0x01: [4-byte CRC32][4-byte compressed_len][zstd compressed data]
//!
//! Files without a marker (older format) are rejected. There is no
//! backward compatibility with pre-marker file formats.

use crate::core::{StorageError, StorageResult};

pub const DEFAULT_PAGE_SIZE: usize = 64 * 1024 - 1;
pub const PAGE_MAGIC: [u8; 4] = *b"PGZC";
pub const COLUMN_FILE_MAGIC: [u8; 8] = *b"GRPHDCOL";
pub const COLUMN_FILE_VERSION: u16 = 1;

const COMPRESSION_MARKER_NONE: u8 = 0x00;
const COMPRESSION_MARKER_ZSTD: u8 = 0x01;

/// Compression type with optional compression level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    Zstd { level: i32 },
}

/// Compress payload with the given strategy.
/// Output format: [1-byte marker][payload]
/// - Marker 0x00: raw data follows
/// - Marker 0x01: [4-byte CRC32][4-byte compressed_len][zstd compressed data]
pub fn compress_payload(data: &[u8], ct: CompressionType) -> StorageResult<Vec<u8>> {
    let mut result = Vec::new();
    let CompressionType::Zstd { level } = ct;
    result.push(COMPRESSION_MARKER_ZSTD);
    let compressed = zstd::encode_all(data, level)
        .map_err(|e| StorageError::io_error(format!("zstd compress failed: {}", e)))?;
    let checksum = crc32fast::hash(&compressed);
    result.extend_from_slice(&checksum.to_le_bytes());
    result.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    result.extend_from_slice(&compressed);
    Ok(result)
}

/// Decompress payload.
/// Accepts only marker 0x00 (raw) or 0x01 (zstd).
/// Rejects anything else — no backward compat with older format.
pub fn decompress_payload(data: &[u8]) -> StorageResult<Vec<u8>> {
    if data.is_empty() {
        return Err(StorageError::deserialize_error(
            "empty data, expected compression marker",
        ));
    }
    match data[0] {
        COMPRESSION_MARKER_NONE => Ok(data[1..].to_vec()),
        COMPRESSION_MARKER_ZSTD => {
            if data.len() < 9 {
                return Err(StorageError::deserialize_error(
                    "truncated compressed data header",
                ));
            }
            let checksum =
                u32::from_le_bytes(data[1..5].try_into().map_err(|_| {
                    StorageError::deserialize_error("failed to read zstd checksum")
                })?);
            let compressed_len = u32::from_le_bytes(data[5..9].try_into().map_err(|_| {
                StorageError::deserialize_error("failed to read zstd compressed length")
            })?) as usize;
            let compressed_end = 9 + compressed_len;
            if compressed_end > data.len() {
                return Err(StorageError::deserialize_error("truncated compressed data"));
            }
            let compressed = &data[9..compressed_end];
            let actual_checksum = crc32fast::hash(compressed);
            if checksum != actual_checksum {
                return Err(StorageError::deserialize_error(
                    "compressed data checksum mismatch",
                ));
            }
            zstd::decode_all(compressed)
                .map_err(|e| StorageError::io_error(format!("zstd decompress failed: {}", e)))
        }
        marker => Err(StorageError::deserialize_error(format!(
            "unknown compression marker: {:#04x}, expected 0x00 or 0x01",
            marker
        ))),
    }
}

/// Compress a file in-place by reading it, compressing, and rewriting.
pub fn compress_file_inplace(path: &std::path::Path, ct: CompressionType) -> StorageResult<()> {
    let data = std::fs::read(path).map_err(|e| {
        StorageError::io_error(format!(
            "failed to read {} for compression: {}",
            path.display(),
            e
        ))
    })?;
    let compressed = compress_payload(&data, ct)?;
    std::fs::write(path, &compressed).map_err(|e| {
        StorageError::io_error(format!(
            "failed to write compressed {}: {}",
            path.display(),
            e
        ))
    })?;
    Ok(())
}

/// Read a file and decompress it.
pub fn read_decompressed(path: &std::path::Path) -> StorageResult<Vec<u8>> {
    let data = std::fs::read(path)
        .map_err(|e| StorageError::io_error(format!("failed to read {}: {}", path.display(), e)))?;
    decompress_payload(&data)
}

pub struct ColumnFileHeader {
    pub page_size: usize,
    pub page_count: u32,
    pub total_rows: u32,
}

impl ColumnFileHeader {
    pub fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&COLUMN_FILE_MAGIC).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write magic: {}", e))
        })?;
        written += 8;
        writer.write_all(&COLUMN_FILE_VERSION.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write version: {}", e))
        })?;
        written += 2;
        let page_size_u16 = self.page_size.min(u16::MAX as usize) as u16;
        writer.write_all(&page_size_u16.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write page_size: {}", e))
        })?;
        written += 2;
        writer.write_all(&self.page_count.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write page_count: {}", e))
        })?;
        written += 4;
        writer.write_all(&self.total_rows.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write total_rows: {}", e))
        })?;
        written += 4;
        let reserved = [0u8; 32];
        writer.write_all(&reserved).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write reserved: {}", e))
        })?;
        written += 32;
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl std::io::Read) -> StorageResult<Self> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read magic: {}", e))
        })?;
        if magic != COLUMN_FILE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid column file magic: {:?}, expected {:?}",
                magic, COLUMN_FILE_MAGIC
            )));
        }
        let mut version_bytes = [0u8; 2];
        reader.read_exact(&mut version_bytes).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read version: {}", e))
        })?;
        let _version = u16::from_le_bytes(version_bytes);
        let mut page_size_bytes = [0u8; 2];
        reader.read_exact(&mut page_size_bytes).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read page_size: {}", e))
        })?;
        let page_size = u16::from_le_bytes(page_size_bytes) as usize;
        let mut page_count_bytes = [0u8; 4];
        reader.read_exact(&mut page_count_bytes).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read page_count: {}", e))
        })?;
        let page_count = u32::from_le_bytes(page_count_bytes);
        let mut total_rows_bytes = [0u8; 4];
        reader.read_exact(&mut total_rows_bytes).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read total_rows: {}", e))
        })?;
        let total_rows = u32::from_le_bytes(total_rows_bytes);
        let mut _reserved = [0u8; 32];
        reader.read_exact(&mut _reserved).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read reserved: {}", e))
        })?;
        Ok(Self {
            page_size,
            page_count,
            total_rows,
        })
    }
}

pub struct PageHeader {
    pub page_size: u32,
    pub compression_type: u8,
    pub crc32: u32,
    pub compressed_len: u32,
}

impl PageHeader {
    pub const SIZE: usize = 15;

    pub fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&PAGE_MAGIC).map_err(|e| {
            StorageError::io_error(format!("PageHeader write magic: {}", e))
        })?;
        written += 4;
        writer.write_all(&self.page_size.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("PageHeader write page_size: {}", e))
        })?;
        written += 4;
        writer.write_all(&[self.compression_type]).map_err(|e| {
            StorageError::io_error(format!("PageHeader write compression_type: {}", e))
        })?;
        written += 1;
        writer.write_all(&self.crc32.to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("PageHeader write crc32: {}", e))
        })?;
        written += 4;
        writer.write_all(&(self.compressed_len as u16).to_le_bytes()).map_err(|e| {
            StorageError::io_error(format!("PageHeader write compressed_len: {}", e))
        })?;
        written += 2;
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl std::io::Read) -> StorageResult<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic).map_err(|e| {
            StorageError::io_error(format!("PageHeader read magic: {}", e))
        })?;
        if magic != PAGE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid page magic: {:?}, expected {:?}",
                magic, PAGE_MAGIC
            )));
        }
        let mut page_size_bytes = [0u8; 4];
        reader.read_exact(&mut page_size_bytes).map_err(|e| {
            StorageError::io_error(format!("PageHeader read page_size: {}", e))
        })?;
        let page_size = u32::from_le_bytes(page_size_bytes);
        let mut compression_type_buf = [0u8; 1];
        reader.read_exact(&mut compression_type_buf).map_err(|e| {
            StorageError::io_error(format!("PageHeader read compression_type: {}", e))
        })?;
        let compression_type = compression_type_buf[0];
        let mut crc32_bytes = [0u8; 4];
        reader.read_exact(&mut crc32_bytes).map_err(|e| {
            StorageError::io_error(format!("PageHeader read crc32: {}", e))
        })?;
        let crc32 = u32::from_le_bytes(crc32_bytes);
        let mut compressed_len_bytes = [0u8; 2];
        reader.read_exact(&mut compressed_len_bytes).map_err(|e| {
            StorageError::io_error(format!("PageHeader read compressed_len: {}", e))
        })?;
        let compressed_len = u16::from_le_bytes(compressed_len_bytes) as u32;
        Ok(Self {
            page_size,
            compression_type,
            crc32,
            compressed_len,
        })
    }
}

pub struct PageWriter {
    page_size: usize,
    compression_level: i32,
    page_count: u32,
}

impl PageWriter {
    pub fn new(page_size: usize, level: i32) -> Self {
        Self {
            page_size,
            compression_level: level,
            page_count: 0,
        }
    }

    pub fn write_page<W: std::io::Write>(
        &mut self,
        writer: &mut W,
        data: &[u8],
    ) -> StorageResult<()> {
        let compression_type = if data.len() < 64 {
            COMPRESSION_MARKER_NONE
        } else {
            COMPRESSION_MARKER_ZSTD
        };
        let (compressed, crc32) = if compression_type == COMPRESSION_MARKER_ZSTD {
            let compressed = zstd::encode_all(data, self.compression_level).map_err(|e| {
                StorageError::io_error(format!("zstd compress failed: {}", e))
            })?;
            let crc = crc32fast::hash(&compressed);
            (compressed, crc)
        } else {
            let crc = crc32fast::hash(data);
            (data.to_vec(), crc)
        };
        let header = PageHeader {
            page_size: data.len() as u32,
            compression_type,
            crc32,
            compressed_len: compressed.len() as u32,
        };
        header.serialize(writer)?;
        writer.write_all(&compressed).map_err(|e| {
            StorageError::io_error(format!("PageWriter write page data: {}", e))
        })?;
        self.page_count += 1;
        Ok(())
    }

    pub fn write_all<W: std::io::Write>(
        &mut self,
        writer: &mut W,
        data: &[u8],
    ) -> StorageResult<()> {
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + self.page_size).min(data.len());
            let chunk = &data[offset..end];
            self.write_page(writer, chunk)?;
            offset = end;
        }
        Ok(())
    }

    pub fn page_count(&self) -> u32 {
        self.page_count
    }
}

pub struct PageReader {
    #[allow(dead_code)]
    page_size: usize,
}

impl PageReader {
    pub fn new(page_size: usize) -> Self {
        Self { page_size }
    }

    pub fn read_page<R: std::io::Read>(&self, reader: &mut R) -> StorageResult<Vec<u8>> {
        let header = PageHeader::deserialize(reader)?;
        let mut compressed = vec![0u8; header.compressed_len as usize];
        reader.read_exact(&mut compressed).map_err(|e| {
            StorageError::io_error(format!("PageReader read compressed data: {}", e))
        })?;
        let actual_crc = crc32fast::hash(&compressed);
        if actual_crc != header.crc32 {
            return Err(StorageError::deserialize_error(
                "Page data CRC32 mismatch".to_string(),
            ));
        }
        let decompressed = if header.compression_type == COMPRESSION_MARKER_ZSTD {
            zstd::decode_all(std::io::Cursor::new(&compressed)).map_err(|e| {
                StorageError::io_error(format!("zstd decompress failed: {}", e))
            })?
        } else {
            compressed
        };
        if decompressed.len() != header.page_size as usize {
            return Err(StorageError::deserialize_error(format!(
                "Page size mismatch: expected {}, got {}",
                header.page_size,
                decompressed.len()
            )));
        }
        Ok(decompressed)
    }

    #[allow(dead_code)]
    pub fn skip_page<R: std::io::Read>(&self, reader: &mut R) -> StorageResult<()> {
        let header = PageHeader::deserialize(reader)?;
        let mut skip_buf = vec![0u8; header.compressed_len as usize];
        reader.read_exact(&mut skip_buf).map_err(|e| {
            StorageError::io_error(format!("PageReader skip page data: {}", e))
        })?;
        Ok(())
    }

    pub fn read_all<R: std::io::Read>(&self, reader: &mut R, page_count: u32) -> StorageResult<Vec<u8>> {
        let mut result = Vec::new();
        for _ in 0..page_count {
            let page = self.read_page(reader)?;
            result.extend_from_slice(&page);
        }
        Ok(result)
    }
}

pub fn write_shadow_file<P: AsRef<std::path::Path>>(
    path: P,
    data: &[u8],
) -> StorageResult<()> {
    let path = path.as_ref();
    let shadow_path = path.with_extension("tmp");
    std::fs::write(&shadow_path, data).map_err(|e| {
        StorageError::io_error(format!(
            "Failed to write shadow file {}: {}",
            shadow_path.display(),
            e
        ))
    })?;
    std::fs::rename(&shadow_path, path).map_err(|e| {
        StorageError::io_error(format!(
            "Failed to rename shadow file {} to {}: {}",
            shadow_path.display(),
            path.display(),
            e
        ))
    })?;
    Ok(())
}

pub fn cleanup_shadow_files<P: AsRef<std::path::Path>>(dir: P) -> StorageResult<usize> {
    let mut cleaned = 0usize;
    let dir = dir.as_ref();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "tmp") {
                std::fs::remove_file(&path).map_err(|e| {
                    StorageError::io_error(format!(
                        "Failed to remove shadow file {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                cleaned += 1;
            }
        }
    }
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_roundtrip_zstd() {
        let data = b"hello world this is a test string for zstd compression";
        let compressed = compress_payload(data, CompressionType::Zstd { level: 3 }).unwrap();
        assert_eq!(compressed[0], COMPRESSION_MARKER_ZSTD);
        let decompressed = decompress_payload(&compressed).unwrap();
        assert_eq!(&decompressed, data);
    }

    #[test]
    fn test_decompress_rejects_unknown_marker() {
        let data = vec![0xFF, 0x01, 0x02, 0x03];
        let result = decompress_payload(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_decompress_rejects_empty() {
        let result = decompress_payload(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_page_write_read_roundtrip() {
        let data = b"test page data for compression";
        let mut buffer = Vec::new();
        let mut writer = PageWriter::new(1024, 3);
        writer.write_page(&mut buffer, data).unwrap();
        let mut cursor = std::io::Cursor::new(&buffer);
        let reader = PageReader::new(1024);
        let result = reader.read_page(&mut cursor).unwrap();
        assert_eq!(&result, data);
    }

    #[test]
    fn test_column_file_header_roundtrip() {
        let header = ColumnFileHeader {
            page_size: 4096,
            page_count: 10,
            total_rows: 1000,
        };
        let mut buffer = Vec::new();
        header.serialize(&mut buffer).unwrap();
        let mut cursor = std::io::Cursor::new(&buffer);
        let result = ColumnFileHeader::deserialize(&mut cursor).unwrap();
        assert_eq!(result.page_size, header.page_size);
        assert_eq!(result.page_count, header.page_count);
        assert_eq!(result.total_rows, header.total_rows);
    }
}

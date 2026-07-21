//! Compression type definition and compression/decompression helpers for storage layer.
//!
//! This module provides the `CompressionType` enum and page-level compression
//! helpers used by the table flush/load pipeline.
//!
//! Every persisted file uses the compression marker format:
//! - Marker 0x00: raw data follows
//! - Marker 0x01: [4-byte CRC32][4-byte compressed_len][zstd compressed data]
//!
//! Files without a marker (older format) are rejected. There is no
//! backward compatibility with pre-marker file formats.

use std::io::Read;

use crate::core::{StorageError, StorageResult};

use crate::storage::safe_read::{BoundedReader, SafeSerializable};

pub const DEFAULT_PAGE_SIZE: usize = 64 * 1024 - 1;
pub const MAX_PAGE_SIZE: usize = 64 * 1024 * 1024;
pub const PAGE_MAGIC: [u8; 4] = *b"PGZC";
pub const COLUMN_FILE_MAGIC: [u8; 8] = *b"GRPHDCOL";
pub const COLUMN_FILE_VERSION: u16 = 2;

const COMPRESSION_MARKER_NONE: u8 = 0x00;
const COMPRESSION_MARKER_ZSTD: u8 = 0x01;

/// Compression type with optional compression level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    Zstd { level: i32 },
}

#[derive(Debug)]
pub struct ColumnFileHeader {
    pub page_size: usize,
    pub page_count: u32,
    pub total_rows: u32,
}

impl ColumnFileHeader {
    pub fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer
            .write_all(&COLUMN_FILE_MAGIC)
            .map_err(|e| StorageError::io_error(format!("ColumnFileHeader write magic: {}", e)))?;
        written += 8;
        writer
            .write_all(&COLUMN_FILE_VERSION.to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("ColumnFileHeader write version: {}", e))
            })?;
        written += 2;
        if self.page_size == 0 || self.page_size > MAX_PAGE_SIZE {
            return Err(StorageError::invalid_input(format!(
                "invalid column page size: {}",
                self.page_size
            )));
        }
        writer
            .write_all(&(self.page_size as u32).to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("ColumnFileHeader write page_size: {}", e))
            })?;
        written += 4;
        writer
            .write_all(&self.page_count.to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("ColumnFileHeader write page_count: {}", e))
            })?;
        written += 4;
        writer
            .write_all(&self.total_rows.to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("ColumnFileHeader write total_rows: {}", e))
            })?;
        written += 4;
        let reserved = [0u8; 30];
        writer.write_all(&reserved).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader write reserved: {}", e))
        })?;
        written += 30;
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl std::io::Read) -> StorageResult<Self> {
        let mut magic = [0u8; 8];
        reader
            .read_exact(&mut magic)
            .map_err(|e| StorageError::io_error(format!("ColumnFileHeader read magic: {}", e)))?;
        if magic != COLUMN_FILE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid column file magic: {:?}, expected {:?}",
                magic, COLUMN_FILE_MAGIC
            )));
        }
        let mut version_bytes = [0u8; 2];
        reader
            .read_exact(&mut version_bytes)
            .map_err(|e| StorageError::io_error(format!("ColumnFileHeader read version: {}", e)))?;
        let version = u16::from_le_bytes(version_bytes);
        if version != COLUMN_FILE_VERSION {
            return Err(StorageError::unsupported_version(
                version as u32,
                COLUMN_FILE_VERSION as u32,
            ));
        }
        let mut page_size_bytes = [0u8; 4];
        reader.read_exact(&mut page_size_bytes).map_err(|e| {
            StorageError::io_error(format!("ColumnFileHeader read page_size: {}", e))
        })?;
        let page_size = u32::from_le_bytes(page_size_bytes) as usize;
        if page_size == 0 || page_size > MAX_PAGE_SIZE {
            return Err(StorageError::deserialize_error(format!(
                "invalid column page size: {}",
                page_size
            )));
        }
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
        let mut _reserved = [0u8; 30];
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
    pub fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer
            .write_all(&PAGE_MAGIC)
            .map_err(|e| StorageError::io_error(format!("PageHeader write magic: {}", e)))?;
        written += 4;
        writer
            .write_all(&self.page_size.to_le_bytes())
            .map_err(|e| StorageError::io_error(format!("PageHeader write page_size: {}", e)))?;
        written += 4;
        writer.write_all(&[self.compression_type]).map_err(|e| {
            StorageError::io_error(format!("PageHeader write compression_type: {}", e))
        })?;
        written += 1;
        writer
            .write_all(&self.crc32.to_le_bytes())
            .map_err(|e| StorageError::io_error(format!("PageHeader write crc32: {}", e)))?;
        written += 4;
        writer
            .write_all(&self.compressed_len.to_le_bytes())
            .map_err(|e| {
                StorageError::io_error(format!("PageHeader write compressed_len: {}", e))
            })?;
        written += 4;
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl std::io::Read) -> StorageResult<Self> {
        let mut magic = [0u8; 4];
        reader
            .read_exact(&mut magic)
            .map_err(|e| StorageError::io_error(format!("PageHeader read magic: {}", e)))?;
        if magic != PAGE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid page magic: {:?}, expected {:?}",
                magic, PAGE_MAGIC
            )));
        }
        let mut page_size_bytes = [0u8; 4];
        reader
            .read_exact(&mut page_size_bytes)
            .map_err(|e| StorageError::io_error(format!("PageHeader read page_size: {}", e)))?;
        let page_size = u32::from_le_bytes(page_size_bytes);
        let mut compression_type_buf = [0u8; 1];
        reader.read_exact(&mut compression_type_buf).map_err(|e| {
            StorageError::io_error(format!("PageHeader read compression_type: {}", e))
        })?;
        let compression_type = compression_type_buf[0];
        let mut crc32_bytes = [0u8; 4];
        reader
            .read_exact(&mut crc32_bytes)
            .map_err(|e| StorageError::io_error(format!("PageHeader read crc32: {}", e)))?;
        let crc32 = u32::from_le_bytes(crc32_bytes);
        let mut compressed_len_bytes = [0u8; 4];
        reader.read_exact(&mut compressed_len_bytes).map_err(|e| {
            StorageError::io_error(format!("PageHeader read compressed_len: {}", e))
        })?;
        let compressed_len = u32::from_le_bytes(compressed_len_bytes);
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
            let compressed = zstd::encode_all(data, self.compression_level)
                .map_err(|e| StorageError::io_error(format!("zstd compress failed: {}", e)))?;
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
        writer
            .write_all(&compressed)
            .map_err(|e| StorageError::io_error(format!("PageWriter write page data: {}", e)))?;
        self.page_count += 1;
        Ok(())
    }

    pub fn write_all<W: std::io::Write>(
        &mut self,
        writer: &mut W,
        data: &[u8],
    ) -> StorageResult<()> {
        if self.page_size == 0 || self.page_size > MAX_PAGE_SIZE {
            return Err(StorageError::invalid_input(format!(
                "invalid page size: {}",
                self.page_size
            )));
        }
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
        if header.page_size == 0 || header.page_size as usize > self.page_size {
            return Err(StorageError::deserialize_error(format!(
                "invalid page size: {}, reader page size: {}",
                header.page_size, self.page_size
            )));
        }
        if !matches!(
            header.compression_type,
            COMPRESSION_MARKER_NONE | COMPRESSION_MARKER_ZSTD
        ) {
            return Err(StorageError::deserialize_error(format!(
                "unknown page compression type: {}",
                header.compression_type
            )));
        }
        let compressed_len = header.compressed_len as usize;
        if compressed_len > MAX_PAGE_SIZE.saturating_mul(2) {
            return Err(StorageError::deserialize_error(format!(
                "compressed page is too large: {}",
                compressed_len
            )));
        }
        let mut bounded = crate::storage::safe_read::BoundedReader::new(reader, compressed_len);
        let mut compressed = vec![0u8; compressed_len];
        bounded.read_exact(&mut compressed).map_err(|e| {
            StorageError::io_error(format!("PageReader read compressed data: {}", e))
        })?;
        if bounded.remaining() != 0 {
            return Err(StorageError::deserialize_error(format!(
                "expected {} bytes of page data, but {} bytes remained",
                compressed_len,
                bounded.remaining()
            )));
        }
        let actual_crc = crc32fast::hash(&compressed);
        if actual_crc != header.crc32 {
            return Err(StorageError::deserialize_error(
                "Page data CRC32 mismatch".to_string(),
            ));
        }
        let decompressed = if header.compression_type == COMPRESSION_MARKER_ZSTD {
            let decoder = zstd::stream::read::Decoder::new(std::io::Cursor::new(&compressed))
                .map_err(|e| StorageError::io_error(format!("zstd decompress failed: {}", e)))?;
            let mut limited = decoder.take(header.page_size as u64 + 1);
            let mut decompressed = Vec::with_capacity(header.page_size as usize);
            std::io::Read::read_to_end(&mut limited, &mut decompressed)
                .map_err(|e| StorageError::io_error(format!("zstd decompress failed: {}", e)))?;
            decompressed
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

    pub fn read_all<R: std::io::Read>(
        &self,
        reader: &mut R,
        page_count: u32,
    ) -> StorageResult<Vec<u8>> {
        let mut result = Vec::new();
        for _ in 0..page_count {
            let page = self.read_page(reader)?;
            result.extend_from_slice(&page);
        }
        Ok(result)
    }

    /// Skip one page without allocating or decompressing its payload.
    pub fn skip_page<R: std::io::Read>(&self, reader: &mut R) -> StorageResult<()> {
        let header = PageHeader::deserialize(reader)?;
        if header.page_size == 0 || header.page_size as usize > self.page_size {
            return Err(StorageError::deserialize_error(format!(
                "invalid page size: {}, reader page size: {}",
                header.page_size, self.page_size
            )));
        }
        if !matches!(
            header.compression_type,
            COMPRESSION_MARKER_NONE | COMPRESSION_MARKER_ZSTD
        ) {
            return Err(StorageError::deserialize_error(format!(
                "unknown page compression type: {}",
                header.compression_type
            )));
        }
        if header.compressed_len as usize > MAX_PAGE_SIZE.saturating_mul(2) {
            return Err(StorageError::deserialize_error(format!(
                "compressed page is too large: {}",
                header.compressed_len
            )));
        }
        let mut bounded = BoundedReader::new(reader, header.compressed_len as usize);
        bounded.skip_all()?;
        if bounded.remaining() != 0 {
            return Err(StorageError::io_error(
                "truncated page payload while skipping".to_string(),
            ));
        }
        Ok(())
    }

    /// Read a page by ordinal. The current format has no offset index, so this
    /// seeks forward page by page while avoiding decompression of skipped pages.
    pub fn read_page_at<R: std::io::Read>(
        &self,
        reader: &mut R,
        page_index: u32,
    ) -> StorageResult<Vec<u8>> {
        for _ in 0..page_index {
            self.skip_page(reader)?;
        }
        self.read_page(reader)
    }

}

pub fn write_shadow_file<P: AsRef<std::path::Path>>(path: P, data: &[u8]) -> StorageResult<()> {
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
            if path.extension().is_some_and(|ext| ext == "tmp") {
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

/// Compress data with zstd and write in spill file format:
/// [1-byte marker][4-byte CRC32][4-byte compressed_len][zstd data]
/// Falls back to uncompressed if compression doesn't reduce size.
pub fn compress_to_writer(writer: &mut impl std::io::Write, data: &[u8], level: i32) -> StorageResult<usize> {
    let compressed = zstd::encode_all(data, level).map_err(|e| {
        StorageError::io_error(format!("zstd compression failed: {}", e))
    })?;

    if compressed.len() < data.len() {
        let crc = crc32fast::hash(&compressed);
        writer.write_all(&[COMPRESSION_MARKER_ZSTD])?;
        writer.write_all(&crc.to_le_bytes())?;
        writer.write_all(&(compressed.len() as u32).to_le_bytes())?;
        writer.write_all(&compressed)?;
        Ok(1 + 4 + 4 + compressed.len())
    } else {
        writer.write_all(&[COMPRESSION_MARKER_NONE])?;
        writer.write_all(data)?;
        Ok(1 + data.len())
    }
}

/// Read data written by `compress_to_writer`, decompressing as needed.
pub fn decompress_from_reader(reader: &mut impl std::io::Read) -> StorageResult<Vec<u8>> {
    let mut marker = [0u8; 1];
    reader.read_exact(&mut marker)?;
    match marker[0] {
        COMPRESSION_MARKER_NONE => {
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            Ok(data)
        }
        COMPRESSION_MARKER_ZSTD => {
            let mut crc_bytes = [0u8; 4];
            reader.read_exact(&mut crc_bytes)?;
            let expected_crc = u32::from_le_bytes(crc_bytes);
            let mut len_bytes = [0u8; 4];
            reader.read_exact(&mut len_bytes)?;
            let compressed_len = u32::from_le_bytes(len_bytes) as usize;
            let mut compressed = vec![0u8; compressed_len];
            reader.read_exact(&mut compressed)?;
            let actual_crc = crc32fast::hash(&compressed);
            if actual_crc != expected_crc {
                return Err(StorageError::data_corruption(format!(
                    "spill file CRC mismatch: expected {expected_crc:#x}, got {actual_crc:#x}"
                )));
            }
            zstd::decode_all(&compressed[..]).map_err(|e| {
                StorageError::io_error(format!("zstd decompression failed: {}", e))
            })
        }
        other => Err(StorageError::deserialize_error(format!(
            "unknown spill compression marker: {other:#x}"
        ))),
    }
}

impl SafeSerializable for PageHeader {
    fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<()> {
        PageHeader::serialize(self, writer).map(|_| ())
    }

    fn deserialize(reader: &mut BoundedReader<'_>) -> StorageResult<Self> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if magic != PAGE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid page magic: {:?}, expected {:?}",
                magic, PAGE_MAGIC
            )));
        }
        let mut buf = [0u8; 13];
        reader.read_exact(&mut buf)?;
        let page_size = u32::from_le_bytes(
            buf[0..4]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid page size bytes"))?,
        );
        let compression_type = buf[4];
        let crc32 = u32::from_le_bytes(
            buf[5..9]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid page crc bytes"))?,
        );
        let compressed_len = u32::from_le_bytes(
            buf[9..13]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid page length bytes"))?,
        );
        Ok(Self {
            page_size,
            compression_type,
            crc32,
            compressed_len,
        })
    }
}

impl SafeSerializable for ColumnFileHeader {
    fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<()> {
        ColumnFileHeader::serialize(self, writer).map(|_| ())
    }

    fn deserialize(reader: &mut BoundedReader<'_>) -> StorageResult<Self> {
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if magic != COLUMN_FILE_MAGIC {
            return Err(StorageError::deserialize_error(format!(
                "Invalid column file magic: {:?}, expected {:?}",
                magic, COLUMN_FILE_MAGIC
            )));
        }
        let mut buf = [0u8; 44];
        reader.read_exact(&mut buf)?;
        let version = u16::from_le_bytes(
            buf[0..2]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid column version bytes"))?,
        );
        if version != COLUMN_FILE_VERSION {
            return Err(StorageError::unsupported_version(
                version as u32,
                COLUMN_FILE_VERSION as u32,
            ));
        }
        let page_size = u32::from_le_bytes(
            buf[2..6]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid column page size bytes"))?,
        ) as usize;
        if page_size == 0 || page_size > MAX_PAGE_SIZE {
            return Err(StorageError::deserialize_error(format!(
                "invalid column page size: {}",
                page_size
            )));
        }
        let page_count = u32::from_le_bytes(
            buf[6..10]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid page count bytes"))?,
        );
        let total_rows = u32::from_le_bytes(
            buf[10..14]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("invalid row count bytes"))?,
        );
        Ok(Self {
            page_size,
            page_count,
            total_rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
use crate::storage::safe_read::BoundedReader;

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

    #[test]
    fn test_column_file_header_rejects_wrong_version() {
        let header = ColumnFileHeader {
            page_size: 4096,
            page_count: 10,
            total_rows: 1000,
        };
        let mut buffer = Vec::new();
        header.serialize(&mut buffer).unwrap();
        let version_offset = 8;
        buffer[version_offset] = 0xFF;
        buffer[version_offset + 1] = 0xFF;
        let mut cursor = std::io::Cursor::new(&buffer);
        let result = ColumnFileHeader::deserialize(&mut cursor);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(
            err.kind(),
            crate::core::error::storage::StorageErrorKind::UnsupportedVersion
        );
    }

    #[test]
    fn test_page_header_safe_serializable_roundtrip() {
        let header = PageHeader {
            page_size: 128,
            compression_type: COMPRESSION_MARKER_ZSTD,
            crc32: 0xDEADBEEF,
            compressed_len: 64,
        };
        let mut buffer = Vec::new();
        header.serialize(&mut buffer).unwrap();
        let mut cursor = std::io::Cursor::new(&buffer);
        let mut bounded = BoundedReader::new(&mut cursor, buffer.len());
        let result = PageHeader::deserialize(&mut bounded).unwrap();
        assert_eq!(result.page_size, header.page_size);
        assert_eq!(result.compression_type, header.compression_type);
        assert_eq!(result.crc32, header.crc32);
        assert_eq!(result.compressed_len, header.compressed_len);
    }

    #[test]
    fn test_page_header_safe_serializable_rejects_truncated() {
        let header = PageHeader {
            page_size: 128,
            compression_type: COMPRESSION_MARKER_ZSTD,
            crc32: 0xDEADBEEF,
            compressed_len: 64,
        };
        let mut buffer = Vec::new();
        header.serialize(&mut buffer).unwrap();
        let truncated = &buffer[..buffer.len() - 2];
        let mut cursor = std::io::Cursor::new(truncated);
        let mut bounded = BoundedReader::new(&mut cursor, truncated.len());
        let result = PageHeader::deserialize(&mut bounded);
        assert!(result.is_err());
    }

    #[test]
    fn test_column_file_header_safe_serializable_rejects_truncated() {
        let header = ColumnFileHeader {
            page_size: 4096,
            page_count: 10,
            total_rows: 1000,
        };
        let mut buffer = Vec::new();
        header.serialize(&mut buffer).unwrap();
        let truncated = &buffer[..buffer.len() - 10];
        let mut cursor = std::io::Cursor::new(truncated);
        let mut bounded = BoundedReader::new(&mut cursor, truncated.len());
        let result = ColumnFileHeader::deserialize(&mut bounded);
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_read_prevents_over_read() {
        let data = [0u8; 100];
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 10);
        let mut buf = [0u8; 20];
        let result = reader.read_exact(&mut buf);
        assert!(result.is_err());
    }
}

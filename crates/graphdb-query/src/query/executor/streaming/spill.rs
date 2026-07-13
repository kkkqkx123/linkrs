//! Spill-to-disk infrastructure for blocking operators.
//!
//! Provides:
//! - `SpillConfig / SpillManager`: temp-file lifecycle management
//! - `SpillWriter / SpillReader / SpilledFile`: binary row serialization
//!   (postcard-based, Variable-length encoding)
//! - `RowBuffer`: a spill-aware `Vec<Vec<Value>>` replacement

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::error::QueryError;
use crate::core::Value;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for disk spill behavior.
#[derive(Debug, Clone)]
pub struct SpillConfig {
    /// Directory for spill files. `None` → system temp dir.
    pub temp_dir: Option<PathBuf>,
    /// Maximum number of spill files per operator instance.
    pub max_spill_files: usize,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            temp_dir: None,
            max_spill_files: 64,
        }
    }
}

// ── File-level types ─────────────────────────────────────────────────────────

/// Metadata for a single spill file on disk.
#[derive(Debug, Clone)]
pub struct SpilledFile {
    pub path: PathBuf,
    pub row_count: u64,
    pub byte_size: u64,
}

// ── SpillManager ─────────────────────────────────────────────────────────────

/// Manages spill-file creation, cleanup, and tracking for one query execution.
///
/// Creates a unique subdirectory under `temp_dir` on construction and removes
/// it (including all spill files) on drop.
#[derive(Debug)]
pub struct SpillManager {
    _config: SpillConfig,
    base_dir: PathBuf,
    file_counter: AtomicU64,
    _spill_bytes: Arc<AtomicU64>,
}

impl SpillManager {
    pub fn new(config: SpillConfig, query_id: u64) -> Result<Self, QueryError> {
        let base = config
            .temp_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("graphdb_spill_{}", query_id));
        std::fs::create_dir_all(&base)
            .map_err(|e| QueryError::execution(format!("Failed to create spill dir: {}", e)))?;
        Ok(Self {
            _config: config,
            base_dir: base,
            file_counter: AtomicU64::new(0),
            _spill_bytes: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn create_writer(&self) -> Result<SpillWriter, QueryError> {
        let id = self.file_counter.fetch_add(1, Ordering::Relaxed);
        let path = self.base_dir.join(format!("spill_{:016x}.bin", id));
        let file = std::fs::File::create(&path)
            .map_err(|e| QueryError::execution(format!("create spill file: {}", e)))?;
        Ok(SpillWriter {
            writer: BufWriter::new(file),
            path,
            row_count: 0,
            byte_size: 0,
        })
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Register recursive cleanup with an execution runtime.
    pub fn register_cleanup(
        &self,
        runtime: &crate::query::executor::streaming::runtime::ExecutionRuntime,
    ) {
        let base = self.base_dir.clone();
        runtime.on_cleanup(move || {
            let _ = std::fs::remove_dir_all(&base);
        });
    }
}

impl Drop for SpillManager {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base_dir);
    }
}

// ── SpillWriter ──────────────────────────────────────────────────────────────

/// Binary writer that serializes rows through postcard.
pub struct SpillWriter {
    writer: BufWriter<std::fs::File>,
    path: PathBuf,
    row_count: u64,
    byte_size: u64,
}

impl SpillWriter {
    pub fn write_row(&mut self, row: &[Value]) -> Result<(), QueryError> {
        let encoded = postcard::to_allocvec(row)
            .map_err(|e| QueryError::execution(format!("spill serialize: {}", e)))?;
        let len = encoded.len() as u64;
        self.writer
            .write_all(&len.to_le_bytes())
            .map_err(|e| QueryError::execution(format!("spill write len: {}", e)))?;
        self.writer
            .write_all(&encoded)
            .map_err(|e| QueryError::execution(format!("spill write data: {}", e)))?;
        self.row_count += 1;
        self.byte_size += 8 + len as u64;
        Ok(())
    }

    pub fn write_rows(&mut self, rows: &[Vec<Value>]) -> Result<(), QueryError> {
        for row in rows {
            self.write_row(row)?;
        }
        Ok(())
    }

    pub fn finalize(mut self) -> Result<SpilledFile, QueryError> {
        self.writer
            .flush()
            .map_err(|e| QueryError::execution(format!("spill flush: {}", e)))?;
        Ok(SpilledFile {
            path: self.path.clone(),
            row_count: self.row_count,
            byte_size: self.byte_size,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn row_count(&self) -> u64 {
        self.row_count
    }
}

impl std::fmt::Debug for SpillWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpillWriter")
            .field("path", &self.path)
            .field("row_count", &self.row_count)
            .field("byte_size", &self.byte_size)
            .finish()
    }
}

// ── SpillReader ──────────────────────────────────────────────────────────────

/// Iterator that reads rows back from a sealed spill file.
pub struct SpillReader {
    reader: BufReader<std::fs::File>,
    _path: PathBuf,
    remaining: u64,
}

impl SpillReader {
    pub fn open(file: &SpilledFile) -> Result<Self, QueryError> {
        let f = std::fs::File::open(&file.path)
            .map_err(|e| QueryError::execution(format!("open spill file: {}", e)))?;
        Ok(Self {
            reader: BufReader::new(f),
            _path: file.path.clone(),
            remaining: file.row_count,
        })
    }

    pub fn read_row(&mut self) -> Result<Option<Vec<Value>>, QueryError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let mut len_buf = [0u8; 8];
        self.reader
            .read_exact(&mut len_buf)
            .map_err(|e| QueryError::execution(format!("spill read len: {}", e)))?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut encoded = vec![0u8; len];
        self.reader
            .read_exact(&mut encoded)
            .map_err(|e| QueryError::execution(format!("spill read data: {}", e)))?;
        let row: Vec<Value> = postcard::from_bytes(&encoded)
            .map_err(|e| QueryError::execution(format!("spill deserialize: {}", e)))?;
        self.remaining -= 1;
        Ok(Some(row))
    }

    pub fn read_all(&mut self) -> Result<Vec<Vec<Value>>, QueryError> {
        let mut rows = Vec::with_capacity(self.remaining as usize);
        while let Some(row) = self.read_row()? {
            rows.push(row);
        }
        Ok(rows)
    }
}

impl std::fmt::Debug for SpillReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpillReader")
            .field("path", &self._path)
            .field("remaining", &self.remaining)
            .finish()
    }
}

// ── RowBuffer ────────────────────────────────────────────────────────────────

/// A spill-aware row buffer that can offload in-memory rows to disk.
///
/// Replace `Vec<Vec<Value>>` in operator state to enable automatic
/// memory-management via disk spill.
#[derive(Debug)]
pub struct RowBuffer {
    rows: Vec<Vec<Value>>,
    spill_files: Vec<SpilledFile>,
    total_rows_in_files: u64,
    total_bytes_in_files: u64,
}

impl RowBuffer {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            spill_files: Vec::new(),
            total_rows_in_files: 0,
            total_bytes_in_files: 0,
        }
    }

    /// Borrow the in-memory rows.
    pub fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    /// Mutable access to the in-memory rows.
    pub fn rows_mut(&mut self) -> &mut Vec<Vec<Value>> {
        &mut self.rows
    }

    /// Push a single row.
    pub fn push(&mut self, row: Vec<Value>) {
        self.rows.push(row);
    }

    /// Extend from an iterator.
    pub fn extend(&mut self, iter: impl IntoIterator<Item = Vec<Value>>) {
        self.rows.extend(iter);
    }

    /// Spill all in-memory rows to a new file. Does nothing when the buffer
    /// has no in-memory data.
    pub fn spill_to_disk(&mut self, manager: &SpillManager) -> Result<(), QueryError> {
        if self.rows.is_empty() {
            return Ok(());
        }
        let mut writer = manager.create_writer()?;
        writer.write_rows(&self.rows)?;
        let file = writer.finalize()?;
        self.total_rows_in_files += self.rows.len() as u64;
        self.total_bytes_in_files += file.byte_size;
        self.spill_files.push(file);
        self.rows.clear();
        Ok(())
    }

    /// Read all rows back (spill files first, then in-memory).
    /// The buffer is left empty after this call.
    pub fn drain_all(&mut self, _manager: &SpillManager) -> Result<Vec<Vec<Value>>, QueryError> {
        let total = self.total_rows() as usize;
        let mut all = Vec::with_capacity(total);
        for sf in &self.spill_files {
            let mut reader = SpillReader::open(sf)?;
            while let Some(row) = reader.read_row()? {
                all.push(row);
            }
        }
        all.append(&mut self.rows);
        self.spill_files.clear();
        self.total_rows_in_files = 0;
        self.total_bytes_in_files = 0;
        Ok(all)
    }

    /// Total rows (in-memory + spilled).
    pub fn total_rows(&self) -> u64 {
        self.rows.len() as u64 + self.total_rows_in_files
    }

    /// Number of rows spilled to disk.
    pub fn spilled_rows(&self) -> u64 {
        self.total_rows_in_files
    }

    /// Bytes spilled to disk.
    pub fn spilled_bytes(&self) -> u64 {
        self.total_bytes_in_files
    }

    pub fn has_spilled(&self) -> bool {
        self.total_rows_in_files > 0
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::NullType;

    fn sample_rows(n: usize) -> Vec<Vec<Value>> {
        (0..n)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("val_{}", i)),
                    Value::Null(NullType::Null),
                    Value::Bool(i % 2 == 0),
                ]
            })
            .collect()
    }

    #[test]
    fn test_spill_writer_reader_roundtrip() {
        let manager =
            SpillManager::new(SpillConfig::default(), 42).unwrap();
        let mut writer = manager.create_writer().unwrap();
        let rows = sample_rows(100);
        writer.write_rows(&rows).unwrap();
        let file = writer.finalize().unwrap();
        assert_eq!(file.row_count, 100);
        assert!(file.byte_size > 0);

        let mut reader = SpillReader::open(&file).unwrap();
        assert_eq!(reader.read_all().unwrap(), rows);
    }

    #[test]
    fn test_spill_empty_file() {
        let manager =
            SpillManager::new(SpillConfig::default(), 99).unwrap();
        let writer = manager.create_writer().unwrap();
        let file = writer.finalize().unwrap();
        assert_eq!(file.row_count, 0);
        let mut reader = SpillReader::open(&file).unwrap();
        assert!(reader.read_row().unwrap().is_none());
    }

    #[test]
    fn test_row_buffer_spill_drain() {
        let manager =
            SpillManager::new(SpillConfig::default(), 101).unwrap();
        let mut buf = RowBuffer::new();
        let rows = sample_rows(50);
        for r in &rows {
            buf.push(r.clone());
        }
        assert!(!buf.has_spilled());
        assert_eq!(buf.total_rows(), 50);

        buf.spill_to_disk(&manager).unwrap();
        assert!(buf.has_spilled());
        assert_eq!(buf.spilled_rows(), 50);
        assert!(buf.rows().is_empty());

        let more = sample_rows(25);
        for r in &more {
            buf.push(r.clone());
        }
        assert_eq!(buf.total_rows(), 75);

        let all = buf.drain_all(&manager).unwrap();
        assert_eq!(all.len(), 75);
        // first 50 from spill file
        assert_eq!(&all[..50], &rows[..]);
        // last 25 from memory
        assert_eq!(&all[50..], &more[..]);
    }
}

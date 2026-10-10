use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::run_format::{
    decode_section, encode_section, fnv1a_64_update, RunCompression, RunHeader, SpilledRun,
    COMPRESSION_MIN_SIZE, FNV1A_64_INIT, RUN_HEADER_SIZE, RUN_VERSION, SECTION_ROWS,
};

// ── RunWriter ────────────────────────────────────────────────────────────────

/// Writes a columnar run to disk with a versioned header, schema fingerprint,
/// and per-section checksums.
///
/// Rows are staged in a columnar batch (bounded by [`SECTION_ROWS`]) and
/// flushed as column-major sections, so writer memory stays flat regardless
/// of run size. Each section is optionally zstd-compressed and individually
/// checksummed; the header is patched in at finalize time.
pub struct RunWriter {
    pub(crate) writer: BufWriter<std::fs::File>,
    pub(crate) path: PathBuf,
    pub(crate) schema_fingerprint: u64,
    pub(crate) row_count: u64,
    pub(crate) section_count: u32,
    pub(crate) body_bytes: u64,
    pub(crate) staging: MaterializedBatch,
}

impl RunWriter {
    /// Create a new run writer with header placeholder already written.
    /// Callers should use `SpillManager::create_run_writer()` instead.
    pub(crate) fn new(
        writer: BufWriter<std::fs::File>,
        path: PathBuf,
        schema_fingerprint: u64,
    ) -> Self {
        Self {
            writer,
            path,
            schema_fingerprint,
            row_count: 0,
            section_count: 0,
            body_bytes: 0,
            staging: MaterializedBatch::new(0, 0),
        }
    }

    /// Write a single row to the run file (staged, flushed per section).
    pub fn write_row(&mut self, row: &[Value]) -> Result<(), QueryError> {
        if self.staging.num_columns() > 0 && row.len() != self.staging.num_columns() {
            return Err(QueryError::execution(format!(
                "spill run: row arity {} != run column count {}",
                row.len(),
                self.staging.num_columns()
            )));
        }
        self.staging.append_row(row.to_vec());
        if self.staging.num_rows() >= SECTION_ROWS {
            self.flush_staging()?;
        }
        Ok(())
    }

    /// Write a batch of rows.
    pub fn write_rows(&mut self, rows: &[Vec<Value>]) -> Result<(), QueryError> {
        for row in rows {
            self.write_row(row)?;
        }
        Ok(())
    }

    /// Write a columnar batch to the run file (staged per section).
    pub fn write_batch(&mut self, batch: &MaterializedBatch) -> Result<(), QueryError> {
        for row in batch.rows() {
            self.write_row(&row)?;
        }
        Ok(())
    }

    /// Encode the staging batch as one columnar frame and append it.
    fn flush_staging(&mut self) -> Result<(), QueryError> {
        if self.staging.is_empty() {
            return Ok(());
        }
        let payload = encode_section(&self.staging)?;
        let (compression, stored) = if payload.len() >= COMPRESSION_MIN_SIZE {
            match zstd::encode_all(payload.as_slice(), 3) {
                Ok(compressed) if compressed.len() < payload.len() => {
                    (RunCompression::Zstd, compressed)
                }
                _ => (RunCompression::None, payload),
            }
        } else {
            (RunCompression::None, payload)
        };
        let flags = compression as u8;
        let checksum = fnv1a_64_update(FNV1A_64_INIT, &[flags]);
        let checksum = fnv1a_64_update(checksum, &stored);
        self.writer
            .write_all(&(stored.len() as u64).to_le_bytes())
            .map_err(|e| QueryError::execution(format!("spill run: write section len: {}", e)))?;
        self.writer
            .write_all(&[flags])
            .map_err(|e| QueryError::execution(format!("spill run: write section flags: {}", e)))?;
        self.writer
            .write_all(&stored)
            .map_err(|e| QueryError::execution(format!("spill run: write section: {}", e)))?;
        self.writer
            .write_all(&checksum.to_le_bytes())
            .map_err(|e| {
                QueryError::execution(format!("spill run: write section checksum: {}", e))
            })?;
        self.body_bytes += 8 + 1 + stored.len() as u64 + 8;
        self.row_count += self.staging.num_rows() as u64;
        self.section_count += 1;
        self.staging.clear();
        Ok(())
    }

    /// Finalize the run: flush staging, patch the header, flush.
    /// Returns `SpilledRun` metadata.
    ///
    /// Low-level primitive: production spill paths should route through
    /// [`SpillManager::finalize_run`] instead so disk quota is enforced and
    /// manager byte counters stay exact. Direct use remains for tests and the
    /// no-manager fallback.
    pub fn finalize(mut self) -> Result<SpilledRun, QueryError> {
        use std::io::Seek;

        self.flush_staging()?;
        self.writer
            .flush()
            .map_err(|e| QueryError::execution(format!("spill run: flush body: {}", e)))?;

        let header = RunHeader {
            version: RUN_VERSION,
            schema_fingerprint: self.schema_fingerprint,
            row_count: self.row_count,
            num_columns: self.staging.num_columns() as u32,
            section_count: self.section_count,
            flags: 0,
        };

        // Seek back to position 0 to overwrite header placeholder with actual header.
        self.writer
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| QueryError::execution(format!("spill run: seek: {}", e)))?;
        let header_bytes = header.encode();
        self.writer
            .write_all(&header_bytes)
            .map_err(|e| QueryError::execution(format!("spill run: write header: {}", e)))?;
        self.writer
            .flush()
            .map_err(|e| QueryError::execution(format!("spill run: flush header: {}", e)))?;

        let file_size = self.body_bytes + RUN_HEADER_SIZE as u64;
        Ok(SpilledRun {
            path: self.path.clone(),
            row_count: self.row_count,
            byte_size: file_size,
            schema_fingerprint: self.schema_fingerprint,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn row_count(&self) -> u64 {
        self.row_count
    }
}

impl std::fmt::Debug for RunWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunWriter")
            .field("path", &self.path)
            .field("row_count", &self.row_count)
            .field("body_bytes", &self.body_bytes)
            .finish()
    }
}

// ── RunReader ────────────────────────────────────────────────────────────────

/// Reads a columnar run file back, validating the header (magic, version,
/// checksum) on open and each section frame (checksum) on decode.
///
/// Sections stream one at a time (bounded by [`SECTION_ROWS`]), so reader
/// memory stays flat regardless of run size.
pub struct RunReader {
    reader: BufReader<std::fs::File>,
    path: PathBuf,
    header: RunHeader,
    remaining: u64,
    section_rows: Vec<Vec<Value>>,
    section_pos: usize,
}

impl RunReader {
    /// Open and validate a run file.
    pub fn open(run: &SpilledRun) -> Result<Self, QueryError> {
        let mut f = std::fs::File::open(&run.path)
            .map_err(|e| QueryError::execution(format!("spill run: open file: {}", e)))?;

        // Read header
        let mut header_buf = [0u8; RUN_HEADER_SIZE as usize];
        f.read_exact(&mut header_buf)
            .map_err(|e| QueryError::execution(format!("spill run: read header: {}", e)))?;
        let header = RunHeader::decode(&header_buf)?;

        // Validate schema fingerprint if provided
        if run.schema_fingerprint != 0 && header.schema_fingerprint != run.schema_fingerprint {
            return Err(QueryError::execution(format!(
                "spill run: schema fingerprint mismatch: expected {}, got {}",
                run.schema_fingerprint, header.schema_fingerprint
            )));
        }

        // Validate row count if provided
        if run.row_count != 0 && header.row_count != run.row_count {
            return Err(QueryError::execution(format!(
                "spill run: row count mismatch: expected {}, got {}",
                run.row_count, header.row_count
            )));
        }

        Ok(Self {
            reader: BufReader::new(f),
            path: run.path.clone(),
            remaining: header.row_count,
            header,
            section_rows: Vec::new(),
            section_pos: 0,
        })
    }

    /// Open a run file with default validation (allows any schema fingerprint).
    pub fn open_path(path: &Path) -> Result<Self, QueryError> {
        let dummy = SpilledRun {
            path: path.to_path_buf(),
            row_count: 0,
            byte_size: 0,
            schema_fingerprint: 0,
        };
        Self::open(&dummy)
    }

    /// Decode the next section frame into the row buffer.
    fn load_next_section(&mut self) -> Result<(), QueryError> {
        let mut len_buf = [0u8; 8];
        self.reader
            .read_exact(&mut len_buf)
            .map_err(|e| QueryError::execution(format!("spill run: read section len: {}", e)))?;
        let payload_len = u64::from_le_bytes(len_buf) as usize;
        if payload_len > 512 * 1024 * 1024 {
            return Err(QueryError::execution(
                "spill run: section payload length out of range".to_string(),
            ));
        }
        let mut flags_buf = [0u8; 1];
        self.reader
            .read_exact(&mut flags_buf)
            .map_err(|e| QueryError::execution(format!("spill run: read section flags: {}", e)))?;
        let mut stored = vec![0u8; payload_len];
        self.reader.read_exact(&mut stored).map_err(|e| {
            QueryError::execution(format!("spill run: read section payload: {}", e))
        })?;
        let mut checksum_buf = [0u8; 8];
        self.reader.read_exact(&mut checksum_buf).map_err(|e| {
            QueryError::execution(format!("spill run: read section checksum: {}", e))
        })?;
        let expected = fnv1a_64_update(FNV1A_64_INIT, &flags_buf);
        let expected = fnv1a_64_update(expected, &stored);
        if expected != u64::from_le_bytes(checksum_buf) {
            return Err(QueryError::execution(
                "spill run: section checksum mismatch".to_string(),
            ));
        }
        let payload: Vec<u8> = match flags_buf[0] {
            0 => stored,
            1 => zstd::decode_all(stored.as_slice())
                .map_err(|e| QueryError::execution(format!("spill run: decompress: {}", e)))?,
            other => {
                return Err(QueryError::execution(format!(
                    "spill run: unknown section compression {}",
                    other
                )));
            }
        };
        let expected_cols = if self.header.num_columns > 0 {
            Some(self.header.num_columns)
        } else {
            None
        };
        let mut batch = decode_section(&payload, expected_cols)?;
        batch.set_fingerprint(self.header.schema_fingerprint);
        self.section_rows = batch.into_rows();
        self.section_pos = 0;
        Ok(())
    }

    /// Read the next row from the run.
    pub fn read_row(&mut self) -> Result<Option<Vec<Value>>, QueryError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.section_pos >= self.section_rows.len() {
            self.load_next_section()?;
        }
        if self.section_pos >= self.section_rows.len() {
            return Err(QueryError::execution(
                "spill run: section row underflow".to_string(),
            ));
        }
        let row = std::mem::take(&mut self.section_rows[self.section_pos]);
        self.section_pos += 1;
        self.remaining -= 1;
        Ok(Some(row))
    }

    /// Read all remaining rows from the run.
    pub fn read_all(&mut self) -> Result<Vec<Vec<Value>>, QueryError> {
        let mut rows = Vec::with_capacity(self.remaining as usize);
        while let Some(row) = self.read_row()? {
            rows.push(row);
        }
        Ok(rows)
    }

    /// Read up to `n` rows into a columnar batch.
    ///
    /// Returns `None` when the run is exhausted. Bounds peak memory to one
    /// output slice instead of the full run.
    pub fn read_batch(&mut self, n: usize) -> Result<Option<MaterializedBatch>, QueryError> {
        if self.remaining == 0 {
            return Ok(None);
        }
        let mut batch = MaterializedBatch::new(0, self.header.schema_fingerprint);
        let mut count = 0usize;
        while count < n {
            match self.read_row()? {
                Some(row) => {
                    batch.append_row(row);
                    count += 1;
                }
                None => break,
            }
        }
        if count == 0 {
            Ok(None)
        } else {
            Ok(Some(batch))
        }
    }

    pub fn header(&self) -> &RunHeader {
        &self.header
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for RunReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunReader")
            .field("path", &self.path)
            .field("remaining", &self.remaining)
            .field("header", &self.header)
            .finish()
    }
}

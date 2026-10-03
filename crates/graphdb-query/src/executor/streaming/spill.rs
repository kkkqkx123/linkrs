//! Spill-to-disk infrastructure for blocking operators.
//!
//! Provides:
//! - `SpillConfig / SpillManager`: temp-file lifecycle management
//! - `SpilledRun / RunWriter / RunReader`: the single spill file format —
//!   columnar v2 run (`GRSC`): versioned header, schema fingerprint,
//!   per-section checksums, optional per-section zstd body (columns are
//!   contiguous postcard-encoded value slices, one section per column group)
//! - `HashPartitionSpiller`: per-partition run writers
//! - `DiskQuota`: separate disk usage tracking for spill operations

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use graphdb_core::columnar::MaterializedBatch;
use graphdb_core::error::QueryError;
use graphdb_core::Value;

// ── Configuration ────────────────────────────────────────────────────────────

/// Configuration for disk spill behavior.
#[derive(Debug, Clone)]
pub struct SpillConfig {
    /// Directory for spill files. `None` → system temp dir.
    pub temp_dir: Option<PathBuf>,
    /// Maximum number of spill files per operator instance.
    pub max_spill_files: usize,
    /// Terminal collector spill threshold in logical rows.
    /// `None` → default threshold; `Some(0)` → collector never spills.
    pub collect_spill_rows: Option<u64>,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            temp_dir: None,
            max_spill_files: 64,
            collect_spill_rows: None,
        }
    }
}

/// Default terminal-collector spill threshold in logical rows.
pub const COLLECTOR_SPILL_ROWS_DEFAULT: u64 = 200_000;

/// Maximum rows per terminal-collector run file. Bounds both the writer-side
/// body buffer and the reader-side full-run load peak.
pub const COLLECTOR_RUN_ROWS_MAX: u64 = 65_536;

/// Default Grace Hash Join partition count.
pub const HASH_JOIN_PARTITIONS_DEFAULT: u64 = 32;

/// Maximum Grace Hash Join repartition depth.
pub const HASH_JOIN_MAX_DEPTH: u32 = 3;

// ── Run file format constants ───────────────────────────────────────────────

/// Magic bytes at the start of every spill run file: `GRSC` = GraphDB Run
/// Spill Columnar. The legacy row-major `GRSP` format is rejected.
const RUN_MAGIC: [u8; 4] = [0x47, 0x52, 0x53, 0x43];

/// Current run file format version (columnar v2).
const RUN_VERSION: u32 = 3;

/// Size of the run file header in bytes.
const RUN_HEADER_SIZE: u32 = 48;

/// Rows staged per columnar section. Bounds writer staging memory and keeps
/// each section independently checksummed and decompressible.
const SECTION_ROWS: usize = 1024;

/// Minimum section payload size (bytes) below which compression is skipped.
const COMPRESSION_MIN_SIZE: usize = 256;

// ── Simple FNV-1a 64-bit checksum ───────────────────────────────────────────

const FNV1A_64_INIT: u64 = 0xcbf29ce484222325;
const FNV1A_64_PRIME: u64 = 0x100000001b3;

fn fnv1a_64(data: &[u8]) -> u64 {
    fnv1a_64_update(FNV1A_64_INIT, data)
}

fn fnv1a_64_update(mut hash: u64, data: &[u8]) -> u64 {
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV1A_64_PRIME);
    }
    hash
}

// ── RunHeader ────────────────────────────────────────────────────────────────

/// Compression marker stored per section frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RunCompression {
    None = 0,
    Zstd = 1,
}

/// Header for a columnar spill run file.
///
/// Layout (48 bytes total):
/// ```text
/// [0..4)   magic: b"GRSC"
/// [4..8)   version: u32 LE (= 3)
/// [8..16)  schema_fingerprint: u64 LE
/// [16..24) row_count: u64 LE
/// [24..28) num_columns: u32 LE
/// [28..32) section_count: u32 LE
/// [32..36) flags: u32 LE (reserved, zero)
/// [36..40) reserved: u32 LE (zero)
/// [40..48) header_checksum: u64 LE (FNV-1a of bytes [0..40))
/// ```
/// Body: `section_count` frames back to back. Each frame:
/// `[payload_len u64 LE][flags u8][payload][checksum u64 LE]`
/// (`flags` bit 0 = zstd; checksum = FNV-1a over flags byte + stored payload
/// bytes). Each (decompressed) payload:
/// `[num_rows u32 LE][num_cols u32 LE]` then per column
/// `[col_len u64 LE][postcard(Vec<Value>) bytes]` — columns contiguous.
/// Values stay postcard self-describing; column types are intentionally not
/// stored (fingerprint + column count validate replay compatibility).
#[derive(Debug, Clone, Copy)]
pub struct RunHeader {
    pub version: u32,
    pub schema_fingerprint: u64,
    pub row_count: u64,
    pub num_columns: u32,
    pub section_count: u32,
    pub flags: u32,
}

impl RunHeader {
    /// Encode header into a 48-byte buffer.
    fn encode(&self) -> [u8; RUN_HEADER_SIZE as usize] {
        let mut buf = [0u8; RUN_HEADER_SIZE as usize];
        buf[0..4].copy_from_slice(&RUN_MAGIC);
        buf[4..8].copy_from_slice(&self.version.to_le_bytes());
        buf[8..16].copy_from_slice(&self.schema_fingerprint.to_le_bytes());
        buf[16..24].copy_from_slice(&self.row_count.to_le_bytes());
        buf[24..28].copy_from_slice(&self.num_columns.to_le_bytes());
        buf[28..32].copy_from_slice(&self.section_count.to_le_bytes());
        buf[32..36].copy_from_slice(&self.flags.to_le_bytes());
        // bytes 36..40 stay zero (reserved)
        let checksum = fnv1a_64(&buf[0..40]);
        buf[40..48].copy_from_slice(&checksum.to_le_bytes());
        buf
    }

    /// Decode header from a 48-byte buffer, validating magic, version, and
    /// header checksum. Legacy row-major files are rejected.
    fn decode(buf: &[u8]) -> Result<Self, QueryError> {
        if buf.len() < RUN_HEADER_SIZE as usize {
            return Err(QueryError::execution(
                "spill run: truncated header".to_string(),
            ));
        }
        if buf[0..4] != RUN_MAGIC {
            return Err(QueryError::execution(
                "spill run: invalid magic bytes".to_string(),
            ));
        }
        let version = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        if version != RUN_VERSION {
            return Err(QueryError::execution(format!(
                "spill run: unsupported version {}, expected {}",
                version, RUN_VERSION
            )));
        }
        let expected = fnv1a_64(&buf[0..40]);
        let actual = u64::from_le_bytes(buf[40..48].try_into().unwrap());
        if expected != actual {
            return Err(QueryError::execution(
                "spill run: header checksum mismatch".to_string(),
            ));
        }
        Ok(Self {
            version,
            schema_fingerprint: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            row_count: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            num_columns: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
            section_count: u32::from_le_bytes(buf[28..32].try_into().unwrap()),
            flags: u32::from_le_bytes(buf[32..36].try_into().unwrap()),
        })
    }
}

/// Encode one staging batch as a columnar section payload (uncompressed).
fn encode_section(batch: &MaterializedBatch) -> Result<Vec<u8>, QueryError> {
    let num_rows = batch.num_rows();
    let num_cols = batch.num_columns();
    let mut payload = Vec::new();
    payload.extend_from_slice(&(num_rows as u32).to_le_bytes());
    payload.extend_from_slice(&(num_cols as u32).to_le_bytes());
    for i in 0..num_cols {
        let encoded = postcard::to_allocvec(batch.column_slice(i))
            .map_err(|e| QueryError::execution(format!("run serialize column: {}", e)))?;
        payload.extend_from_slice(&(encoded.len() as u64).to_le_bytes());
        payload.extend_from_slice(&encoded);
    }
    Ok(payload)
}

/// Decode one section payload into a batch, validating column count.
fn decode_section(
    payload: &[u8],
    expected_cols: Option<u32>,
) -> Result<MaterializedBatch, QueryError> {
    let fail = |msg: String| QueryError::execution(format!("spill run: {msg}"));
    if payload.len() < 8 {
        return Err(fail("truncated section payload".to_string()));
    }
    let num_rows = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    let num_cols = u32::from_le_bytes(payload[4..8].try_into().unwrap());
    if num_rows > 10_000_000 || num_cols > 100_000 {
        return Err(fail("section dimensions out of range".to_string()));
    }
    if let Some(expected) = expected_cols {
        if num_cols != expected {
            return Err(fail(format!(
                "section column count {} != run column count {}",
                num_cols, expected
            )));
        }
    }
    let mut offset = 8usize;
    let mut batch = MaterializedBatch::new(0, 0);
    for _ in 0..num_cols {
        if payload.len() < offset + 8 {
            return Err(fail("truncated section column".to_string()));
        }
        let col_len = u64::from_le_bytes(payload[offset..offset + 8].try_into().unwrap()) as usize;
        offset += 8;
        if payload.len() < offset + col_len {
            return Err(fail("truncated section column data".to_string()));
        }
        let values: Vec<Value> = postcard::from_bytes(&payload[offset..offset + col_len])
            .map_err(|e| QueryError::execution(format!("spill run: deserialize column: {}", e)))?;
        offset += col_len;
        if values.len() != num_rows {
            return Err(fail(format!(
                "section column length {} != section row count {}",
                values.len(),
                num_rows
            )));
        }
        batch.append_column(values);
    }
    Ok(batch)
}

// ── Metadata for a spill file with enhanced run format ───────────────────────

/// Metadata for a single spill run file (sorted spill).
#[derive(Debug, Clone)]
pub struct SpilledRun {
    pub path: PathBuf,
    pub row_count: u64,
    pub byte_size: u64,
    pub schema_fingerprint: u64,
}

/// Schema fingerprint for a run file: FNV-style hash over column names.
///
/// Single implementation shared by every run writer (sort runs, set-operator
/// spill); readers validate it against the recorded fingerprint on open.
pub fn schema_fingerprint(col_names: &[String]) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for name in col_names {
        hasher.write(name.as_bytes());
        hasher.write_u8(0);
    }
    hasher.finish()
}

// ── DiskQuota (separate from memory budget) ──────────────────────────────────

/// Tracks disk space used by spill operations for a single query.
///
/// Disk quota is independent of the memory budget.  Exceeding disk quota
/// produces a structured error rather than a silent spill-to-nowhere.
#[derive(Debug, Clone)]
pub struct DiskQuota {
    max_bytes: u64,
    used: Arc<AtomicU64>,
}

impl DiskQuota {
    /// Create a quota with the given byte limit.  `0` = unlimited.
    pub fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            used: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Default quota: 2 GiB per query.
    pub fn default_quota() -> Self {
        Self::new(2 * 1024 * 1024 * 1024)
    }

    /// Try to reserve `bytes` of disk space.  Returns an error when the
    /// quota would be exceeded.
    pub fn try_reserve(&self, bytes: u64) -> Result<(), QueryError> {
        if self.max_bytes == 0 {
            // unlimited
            let _ = self.used.fetch_add(bytes, Ordering::Relaxed);
            return Ok(());
        }
        let mut prev = self.used.load(Ordering::Relaxed);
        loop {
            let total = prev.checked_add(bytes).ok_or_else(|| {
                QueryError::execution(format!(
                    "Disk quota overflow: request {} bytes overflows u64",
                    bytes,
                ))
            })?;
            if total > self.max_bytes {
                return Err(QueryError::execution(format!(
                    "Disk quota exceeded: request {} bytes, total {} > quota {} bytes",
                    bytes, total, self.max_bytes,
                )));
            }
            match self
                .used
                .compare_exchange_weak(prev, total, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(current) => prev = current,
            }
        }
    }

    /// Release `bytes` of disk space.
    pub fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }

    /// Current used disk space.
    pub fn current(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Maximum allowed disk space.
    pub fn max(&self) -> u64 {
        self.max_bytes
    }
}

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

// ── Hash partition spill ─────────────────────────────────────────────────────

/// Fixed hash algorithm for partition spill operations.
///
/// Uses a simple FNV-1a hash of the serialized row/key data.
/// The seed and algorithm version are part of the contract—changing
/// them will change partition assignments across all operators.
pub const HASH_PARTITION_VERSION: u32 = 1;
pub const HASH_PARTITION_SEED: u64 = 0xdeadbeefcafe;

/// Compute the partition index for a row of values.
///
/// Used by hash join, hash aggregate, and hash distinct to
/// distribute rows across partitions.
pub fn hash_row_partition(row: &[Value], num_partitions: u64) -> u64 {
    // Serialize the row to bytes and hash
    let encoded = postcard::to_allocvec(row).unwrap_or_default();
    let hash = fnv1a_64_update(HASH_PARTITION_SEED, &encoded);
    hash % num_partitions
}

/// Compute the partition index for pre-serialized key bytes.
///
/// Shared primitive behind [`hash_row_partition`] and join-key partitioning
/// so both sides of a Grace Hash Join agree on partition assignment.
pub fn hash_bytes_partition(bytes: &[u8], num_partitions: u64) -> u64 {
    if num_partitions == 0 {
        return 0;
    }
    fnv1a_64_update(HASH_PARTITION_SEED, bytes) % num_partitions
}

/// Compute one partition index per batch row over a column subset.
///
/// Key-column projection of [`hash_row_partition`]: rows are hashed by the
/// selected columns only (empty `cols` means the full row), so Distinct and
/// Aggregate spill by key instead of by full-row bytes.
pub fn hash_column_partition(
    batch: &MaterializedBatch,
    cols: &[usize],
    num_partitions: u64,
) -> Vec<u64> {
    batch
        .hash_rows(cols)
        .into_iter()
        .map(|h| {
            if num_partitions == 0 {
                0
            } else {
                h % num_partitions
            }
        })
        .collect()
}

/// Configuration for hash-based partition spill.
#[derive(Debug, Clone)]
pub struct HashPartitionConfig {
    /// Number of partitions to create.
    pub num_partitions: u64,
    /// Maximum rows per partition before triggering recursive repartition.
    pub max_rows_per_partition: u64,
    /// Maximum recursion depth for skew handling.
    pub max_recursion_depth: u32,
}

impl Default for HashPartitionConfig {
    fn default() -> Self {
        Self {
            num_partitions: 16,
            max_rows_per_partition: 1_000_000,
            max_recursion_depth: 3,
        }
    }
}

/// A partition spill writer that routes rows by hash into separate files.
///
/// Each partition gets its own `RunWriter` for sorted/typed row data.
/// The spiller handles skew detection and recursive repartitioning.
#[derive(Debug)]
pub struct HashPartitionSpiller {
    config: HashPartitionConfig,
    writers: Vec<Option<RunWriter>>,
    counts: Vec<u64>,
    recursion_depth: u32,
    schema_fingerprint: u64,
}

impl HashPartitionSpiller {
    /// Create a new hash partition spiller.
    pub fn new(
        config: HashPartitionConfig,
        manager: &SpillManager,
        schema_fingerprint: u64,
    ) -> Result<Self, QueryError> {
        let n = config.num_partitions as usize;
        let mut writers = Vec::with_capacity(n);
        for _ in 0..n {
            writers.push(Some(manager.create_run_writer(schema_fingerprint)?));
        }
        Ok(Self {
            config,
            writers,
            counts: vec![0; n],
            recursion_depth: 0,
            schema_fingerprint,
        })
    }

    /// Insert a row into the appropriate partition.
    pub fn insert_row(&mut self, row: &[Value], manager: &SpillManager) -> Result<(), QueryError> {
        let partition = hash_row_partition(row, self.config.num_partitions) as usize;
        self.insert_row_to_partition(row, partition, manager)
    }

    /// Insert a row into a specific partition (caller computes the hash).
    ///
    /// Useful when the caller needs to partition by a derived key
    /// (e.g., group key for aggregate) rather than the row itself.
    pub fn insert_row_to_partition(
        &mut self,
        row: &[Value],
        partition: usize,
        manager: &SpillManager,
    ) -> Result<(), QueryError> {
        if let Some(Some(writer)) = self.writers.get_mut(partition) {
            writer.write_row(row)?;
            self.counts[partition] += 1;

            if self.counts[partition] > self.config.max_rows_per_partition
                && self.recursion_depth < self.config.max_recursion_depth
            {
                self.repartition(manager)?;
            }
        }
        Ok(())
    }

    /// Finalize all partitions through the manager so disk quota is enforced.
    pub fn finalize_with_manager(
        mut self,
        manager: &SpillManager,
    ) -> Result<Vec<Option<SpilledRun>>, QueryError> {
        let mut runs = Vec::with_capacity(self.writers.len());
        for writer in self.writers.drain(..) {
            match writer {
                Some(w) => runs.push(Some(manager.finalize_run(w)?)),
                None => runs.push(None),
            }
        }
        Ok(runs)
    }

    /// Recursively repartition when skew is detected.
    ///
    /// This splits the overflowing partition into sub-partitions by
    /// re-hashing with an increased partition count. Intermediate runs go
    /// through [`SpillManager::finalize_run`] so disk quota applies; replaced
    /// files are unlinked and their reservations released.
    fn repartition(&mut self, manager: &SpillManager) -> Result<(), QueryError> {
        self.recursion_depth += 1;
        let _old_count = self.config.num_partitions;
        self.config.num_partitions = self.config.num_partitions.saturating_mul(2);

        // Finalize current writers to get run files (quota-checked).
        let old_runs = self.finalize_current(manager)?;

        // Create new writers for doubled partitions
        let mut new_writers = Vec::with_capacity(self.config.num_partitions as usize);
        for _ in 0..self.config.num_partitions {
            new_writers.push(Some(manager.create_run_writer(self.schema_fingerprint)?));
        }
        let mut new_counts = vec![0u64; self.config.num_partitions as usize];

        // Read back and rehash all old partitions into new partitions
        for old_run in old_runs.into_iter().flatten() {
            let byte_size = old_run.byte_size;
            let mut reader = RunReader::open(&old_run)?;
            while let Some(row) = reader.read_row()? {
                let partition = hash_row_partition(&row, self.config.num_partitions) as usize;
                if let Some(Some(writer)) = new_writers.get_mut(partition) {
                    writer.write_row(&row)?;
                    new_counts[partition] += 1;
                }
            }
            // Delete old partition file and release its quota reservation:
            // the data now lives in the new partitions.
            let _ = std::fs::remove_file(&old_run.path);
            manager.disk_quota().release(byte_size);
        }

        self.writers = new_writers;
        self.counts = new_counts;
        Ok(())
    }

    /// Finalize all current partition writers and return the run metadata.
    ///
    /// Quota-checked via [`SpillManager::finalize_run`]; intermediate
    /// repartition files are accounted exactly like final runs.
    fn finalize_current(
        &mut self,
        manager: &SpillManager,
    ) -> Result<Vec<Option<SpilledRun>>, QueryError> {
        let mut runs = Vec::with_capacity(self.writers.len());
        for writer in self.writers.drain(..) {
            match writer {
                Some(w) => runs.push(Some(manager.finalize_run(w)?)),
                None => runs.push(None),
            }
        }
        Ok(runs)
    }

    /// Current partition row counts.
    pub fn partition_counts(&self) -> &[u64] {
        &self.counts
    }

    /// Number of partitions.
    pub fn num_partitions(&self) -> u64 {
        self.config.num_partitions
    }
}

// ── SpillManager ─────────────────────────────────────────────────────────────

/// Manages spill-file creation, cleanup, and tracking for one query execution.
///
/// Creates a unique subdirectory under `temp_dir` on construction and removes
/// it (including all spill files) on drop.
#[derive(Debug)]
pub struct SpillManager {
    pub(crate) config: SpillConfig,
    pub(crate) base_dir: PathBuf,
    pub(crate) file_counter: AtomicU64,
    pub(crate) spill_bytes: Arc<AtomicU64>,
    pub(crate) disk_quota: DiskQuota,
}

impl SpillManager {
    pub fn new(config: SpillConfig, query_id: u64) -> Result<Self, QueryError> {
        Self::new_with_quota(config, query_id, DiskQuota::default_quota())
    }

    /// Create with explicit disk quota.
    pub fn new_with_quota(
        config: SpillConfig,
        query_id: u64,
        disk_quota: DiskQuota,
    ) -> Result<Self, QueryError> {
        let base = config
            .temp_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("graphdb_spill_{}", query_id));
        std::fs::create_dir_all(&base)
            .map_err(|e| QueryError::execution(format!("Failed to create spill dir: {}", e)))?;
        Ok(Self {
            config,
            base_dir: base,
            file_counter: AtomicU64::new(0),
            spill_bytes: Arc::new(AtomicU64::new(0)),
            disk_quota,
        })
    }

    /// Create a run writer for spill data (versioned format with
    /// header/checksum). This is the single spill file format.
    ///
    /// Enforces `max_spill_files`: once the file budget is exhausted no new
    /// run can be created and a structured error is returned.
    pub fn create_run_writer(&self, schema_fingerprint: u64) -> Result<RunWriter, QueryError> {
        let id = self.file_counter.fetch_add(1, Ordering::Relaxed);
        if id >= self.config.max_spill_files as u64 {
            return Err(QueryError::execution(format!(
                "spill run: too many spill files (limit {})",
                self.config.max_spill_files,
            )));
        }
        let path = self.base_dir.join(format!("run_{:016x}.run", id));
        let file = std::fs::File::create(&path)
            .map_err(|e| QueryError::execution(format!("create run file: {}", e)))?;

        // Reserve header space by writing dummy bytes
        let mut writer = BufWriter::new(file);
        let header_placeholder = [0u8; RUN_HEADER_SIZE as usize];
        writer
            .write_all(&header_placeholder)
            .map_err(|e| QueryError::execution(format!("write run header placeholder: {}", e)))?;

        Ok(RunWriter::new(writer, path, schema_fingerprint))
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Access the spill configuration.
    pub fn config(&self) -> &SpillConfig {
        &self.config
    }

    /// Total spilled bytes finalized through this manager.
    pub fn spilled_bytes(&self) -> u64 {
        self.spill_bytes.load(Ordering::Relaxed)
    }

    /// Effective terminal-collector spill threshold in logical rows.
    ///
    /// `SpillConfig::collect_spill_rows`: `None` selects the default,
    /// `Some(0)` disables collector spill.
    pub fn collector_spill_threshold(&self) -> u64 {
        match self.config.collect_spill_rows {
            None => COLLECTOR_SPILL_ROWS_DEFAULT,
            Some(0) => u64::MAX,
            Some(v) => v,
        }
    }

    /// Finalize a run while enforcing disk quota.
    ///
    /// Single choke point for all spill paths: the run is finalized first
    /// (its exact on-disk size is only known then), quota is reserved, and
    /// on quota failure the file is removed so no orphaned run is left
    /// behind. Callers must route new `RunWriter::finalize` uses through
    /// here; direct `finalize` remains for tests and legacy paths.
    pub fn finalize_run(&self, writer: RunWriter) -> Result<SpilledRun, QueryError> {
        let path = writer.path().to_path_buf();
        let run = writer.finalize()?;
        if let Err(e) = self.disk_quota.try_reserve(run.byte_size) {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        self.spill_bytes.fetch_add(run.byte_size, Ordering::Relaxed);
        Ok(run)
    }

    /// Access the disk quota.
    pub fn disk_quota(&self) -> &DiskQuota {
        &self.disk_quota
    }

    /// Register recursive cleanup with an execution runtime.
    pub fn register_cleanup(
        &self,
        runtime: &crate::executor::streaming::runtime::ExecutionRuntime,
    ) {
        let base = self.base_dir.clone();
        runtime.on_cleanup(move || {
            let _ = std::fs::remove_dir_all(&base);
        });
    }
}

/// Finalize a partition spiller through the runtime's spill manager.
///
/// Quota-enforcing choke point for partition spill paths (aggregate, window,
/// group-by, distinct/materialize, join): when a manager is present each run
/// goes through [`SpillManager::finalize_run`] and the run sizes are recorded
/// into the runtime [`ColumnarStats`](crate::executor::streaming::runtime::ColumnarStats).
/// Without a manager there is no quota to enforce, so runs are finalized
/// directly.
pub fn finalize_partitions_with_runtime(
    mut spiller: HashPartitionSpiller,
    runtime: Option<&Arc<crate::executor::streaming::runtime::ExecutionRuntime>>,
) -> Result<Vec<Option<SpilledRun>>, QueryError> {
    match runtime.and_then(|rt| rt.get_spill_manager()) {
        Some(sm) => {
            let runs = spiller.finalize_with_manager(&sm)?;
            if let Some(rt) = runtime {
                let stats = rt.columnar_stats();
                for run in runs.iter().flatten() {
                    stats.record_spill(run.row_count, run.byte_size);
                }
            }
            Ok(runs)
        }
        None => {
            let mut runs = Vec::with_capacity(spiller.writers.len());
            for writer in spiller.writers.drain(..) {
                match writer {
                    Some(w) => runs.push(Some(w.finalize()?)),
                    None => runs.push(None),
                }
            }
            Ok(runs)
        }
    }
}

impl Drop for SpillManager {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base_dir);
    }
}

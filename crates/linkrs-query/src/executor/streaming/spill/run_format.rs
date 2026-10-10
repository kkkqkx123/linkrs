use std::path::PathBuf;

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

// ── Run file format constants ───────────────────────────────────────────────

/// Magic bytes at the start of every spill run file: `LRSC` = Linkrs Run
pub(super) const RUN_MAGIC: [u8; 4] = [0x4C, 0x52, 0x53, 0x43];

/// Current run file format version (columnar v2).
pub(super) const RUN_VERSION: u32 = 3;

/// Size of the run file header in bytes.
pub(super) const RUN_HEADER_SIZE: u32 = 48;

/// Rows staged per columnar section. Bounds writer staging memory and keeps
/// each section independently checksummed and decompressible.
pub(super) const SECTION_ROWS: usize = 1024;

/// Minimum section payload size (bytes) below which compression is skipped.
pub(super) const COMPRESSION_MIN_SIZE: usize = 256;

// ── Simple FNV-1a 64-bit checksum ───────────────────────────────────────────

pub(super) const FNV1A_64_INIT: u64 = 0xcbf29ce484222325;
const FNV1A_64_PRIME: u64 = 0x100000001b3;

fn fnv1a_64(data: &[u8]) -> u64 {
    fnv1a_64_update(FNV1A_64_INIT, data)
}

pub(super) fn fnv1a_64_update(mut hash: u64, data: &[u8]) -> u64 {
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
/// [0..4)   magic: b"LRSC"
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
    pub(super) fn encode(&self) -> [u8; RUN_HEADER_SIZE as usize] {
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
    pub(super) fn decode(buf: &[u8]) -> Result<Self, QueryError> {
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
pub(super) fn encode_section(batch: &MaterializedBatch) -> Result<Vec<u8>, QueryError> {
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
pub(super) fn decode_section(
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

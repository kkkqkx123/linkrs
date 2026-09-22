use std::path::{Path, PathBuf};

use graphdb_core::{StorageError, StorageResult};

use crate::edge::ImmutableCsr;

/// Snapshot file magic: `b"GCSR"`.
pub(crate) const SNAPSHOT_MAGIC: u32 = 0x52534347;
/// Header bytes: magic + rows + entries + edge count + flags + seven
/// `(offset, length)` descriptors.
pub(crate) const SNAPSHOT_HEADER_LEN: usize = 4 + 32 + 7 * 16;
/// Trailing checksum bytes covering the header plus all columns.
pub(crate) const SNAPSHOT_CRC_LEN: usize = 4;
/// Flags word bit marking a snapshot that carries bundled inline values.
pub(crate) const SNAPSHOT_FLAG_VALUED: u64 = 1;

pub(crate) fn snapshot_error(message: String) -> StorageError {
    StorageError::deserialize_error(message)
}

/// Sibling snapshot-file path for a group base file.
pub fn snapshot_path_for(base: &Path) -> PathBuf {
    let mut name = base.as_os_str().to_owned();
    name.push(".snapshot");
    base.with_file_name(name)
}

/// Byte range of one flat column inside the snapshot file.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ColumnRange {
    pub(crate) start: usize,
    pub(crate) len: usize,
}

impl ColumnRange {
    pub(crate) fn end(&self) -> usize {
        self.start.saturating_add(self.len)
    }
}

/// Column ranges of a validated snapshot file.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SnapshotColumns {
    pub(crate) degrees: ColumnRange,
    pub(crate) endpoints: ColumnRange,
    pub(crate) ranks: ColumnRange,
    pub(crate) edge_ids: ColumnRange,
    pub(crate) deletes: ColumnRange,
    pub(crate) values: ColumnRange,
    pub(crate) validity: ColumnRange,
}

pub(crate) fn read_u32_le_at(bytes: &[u8], offset: usize) -> StorageResult<u32> {
    let end = offset.saturating_add(4);
    let chunk = bytes
        .get(offset..end)
        .ok_or_else(|| snapshot_error(format!("snapshot file truncated at byte {offset}")))?;
    Ok(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
}

pub(crate) fn read_u64_le_at(bytes: &[u8], offset: usize) -> StorageResult<u64> {
    let end = offset.saturating_add(8);
    let chunk = bytes
        .get(offset..end)
        .ok_or_else(|| snapshot_error(format!("snapshot file truncated at byte {offset}")))?;
    Ok(u64::from_le_bytes([
        chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
    ]))
}

/// Parse and validate the snapshot header, returning row/entry counts, the
/// stored live edge count and the column ranges.
pub(crate) fn parse_header(bytes: &[u8]) -> StorageResult<(usize, usize, u64, SnapshotColumns)> {
    if bytes.len() < SNAPSHOT_HEADER_LEN + SNAPSHOT_CRC_LEN {
        return Err(snapshot_error(format!(
            "snapshot file too short for header: {} bytes",
            bytes.len()
        )));
    }
    let (body, trailer) = bytes.split_at(bytes.len() - SNAPSHOT_CRC_LEN);
    let stored = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let computed = crc32fast::hash(body);
    if stored != computed {
        return Err(snapshot_error(format!(
            "snapshot file CRC mismatch: stored={stored:#x} computed={computed:#x}"
        )));
    }
    let bytes = body;
    let magic = read_u32_le_at(bytes, 0)?;
    if magic != SNAPSHOT_MAGIC {
        return Err(snapshot_error(format!(
            "snapshot file magic mismatch: {magic:#010x}"
        )));
    }
    let rows = read_u64_le_at(bytes, 4)? as usize;
    let entries = read_u64_le_at(bytes, 12)? as usize;
    let edge_count = read_u64_le_at(bytes, 20)?;
    let flags = read_u64_le_at(bytes, 28)?;
    if flags & !SNAPSHOT_FLAG_VALUED != 0 {
        return Err(snapshot_error(format!(
            "snapshot file flags out of range: {flags:#x}"
        )));
    }
    let valued = flags & SNAPSHOT_FLAG_VALUED != 0;
    let mut cursor = 36usize;
    let mut ranges = [ColumnRange::default(); 7];
    for range in &mut ranges {
        let start = read_u64_le_at(bytes, cursor)? as usize;
        let len = read_u64_le_at(bytes, cursor + 8)? as usize;
        *range = ColumnRange { start, len };
        cursor += 16;
    }
    let expected = [
        rows.saturating_mul(4),
        entries.saturating_mul(4),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        if valued { entries.saturating_mul(8) } else { 0 },
        if valued { entries.div_ceil(8) } else { 0 },
    ];
    let mut poll_end = SNAPSHOT_HEADER_LEN;
    for (range, want) in ranges.iter().zip(expected) {
        if range.len != want {
            return Err(snapshot_error(format!(
                "snapshot column length mismatch: holds {} bytes, layout needs {want}",
                range.len
            )));
        }
        if range.start != poll_end {
            return Err(snapshot_error(format!(
                "snapshot column gap or overlap at byte {}",
                range.start
            )));
        }
        poll_end = range.end();
    }
    if bytes.len() != poll_end {
        return Err(snapshot_error(format!(
            "snapshot file size mismatch: holds {} bytes, layout needs {poll_end}",
            bytes.len()
        )));
    }
    let columns = SnapshotColumns {
        degrees: ranges[0],
        endpoints: ranges[1],
        ranks: ranges[2],
        edge_ids: ranges[3],
        deletes: ranges[4],
        values: ranges[5],
        validity: ranges[6],
    };
    Ok((rows, entries, edge_count, columns))
}

/// Write the snapshot file for one frozen group.
///
/// Encodes the packed columns flat and swaps the file in atomically through
/// a sibling temp file, so concurrent readers never observe a partial file.
/// Valued frozen groups carry two trailing columns (value words plus
/// validity bytes); unvalued groups store empty ranges for both.
pub fn write_snapshot_file(frozen: &ImmutableCsr, path: &Path) -> StorageResult<()> {
    let hot = frozen.packed_hot();
    let cold = frozen.packed_cold();
    let degrees = frozen.packed_degrees();
    debug_assert_eq!(hot.len(), cold.len());
    let rows = degrees.len();
    let entries = hot.len();
    let valued = frozen.has_valued_entries();
    let flags = if valued { SNAPSHOT_FLAG_VALUED } else { 0 };
    let mut ranges = [ColumnRange::default(); 7];
    let mut cursor = SNAPSHOT_HEADER_LEN;
    let lens = [
        rows.saturating_mul(4),
        entries.saturating_mul(4),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        entries.saturating_mul(8),
        if valued { entries.saturating_mul(8) } else { 0 },
        if valued { entries.div_ceil(8) } else { 0 },
    ];
    for (range, len) in ranges.iter_mut().zip(lens) {
        *range = ColumnRange { start: cursor, len };
        cursor += len;
    }
    let mut bytes = Vec::with_capacity(cursor);
    bytes.extend_from_slice(&SNAPSHOT_MAGIC.to_le_bytes());
    bytes.extend_from_slice(&(rows as u64).to_le_bytes());
    bytes.extend_from_slice(&(entries as u64).to_le_bytes());
    bytes.extend_from_slice(&frozen.edge_count().to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    for range in &ranges {
        bytes.extend_from_slice(&(range.start as u64).to_le_bytes());
        bytes.extend_from_slice(&(range.len as u64).to_le_bytes());
    }
    for degree in degrees {
        bytes.extend_from_slice(&degree.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.endpoint.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.rank.to_le_bytes());
    }
    for hot_nbr in hot {
        bytes.extend_from_slice(&hot_nbr.edge_id.0.to_le_bytes());
    }
    for cold_stamps in cold {
        bytes.extend_from_slice(&cold_stamps.delete_ts.to_le_bytes());
    }
    if valued {
        for value in frozen.packed_values() {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(frozen.packed_valid_bytes());
    }
    debug_assert_eq!(bytes.len(), cursor);
    let crc = crc32fast::hash(&bytes);
    bytes.extend_from_slice(&crc.to_le_bytes());
    let tmp = path.with_extension("snapshot.tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| StorageError::io_error(format!("snapshot file write failed: {e}")))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| StorageError::io_error(format!("snapshot file swap failed: {e}")))?;
    Ok(())
}

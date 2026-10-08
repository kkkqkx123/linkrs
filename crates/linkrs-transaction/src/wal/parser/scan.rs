//! Byte-scan engine: header decode, LSN chain validation, length /
//! op-type / checksum validation, compression decode and record reassembly.

use linkrs_core::wal::types::{
    Lsn, RecordType, WalCompression, WalError, WalHeader, WalOpType, WalRecoveryMode, WalResult,
    WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE,
};

use super::fragment::FragmentBuffer;
use super::types::{ParsedWalEntry, RecoveryResult};

/// Scan one whole WAL file buffer into a recovery result.
pub(super) fn parse_wal_file_bytes(
    buffer: &[u8],
    file_start_lsn: Lsn,
    recovery_mode: WalRecoveryMode,
    verify_checksum: bool,
) -> WalResult<RecoveryResult> {
    let mut result = RecoveryResult::default();
    let mut fragment_buffer = FragmentBuffer::new();
    let mut expected_prev_lsn = file_start_lsn;

    let mut offset = WAL_FILE_HEADER_SIZE;
    while offset + WAL_HEADER_SIZE <= buffer.len() {
        let header = match WalHeader::from_bytes(&buffer[offset..offset + WAL_HEADER_SIZE]) {
            Some(h) => h,
            None => match recovery_mode {
                WalRecoveryMode::AbortOnCorruption => {
                    return Err(WalError::Corrupted(format!(
                        "Invalid WAL header at offset {}",
                        offset
                    )));
                }
                _ => {
                    result.corrupted_count += 1;
                    offset += 1;
                    continue;
                }
            },
        };

        if is_zero_padding(&header) {
            break;
        }

        validate_lsn_chain(
            &header,
            expected_prev_lsn,
            file_start_lsn,
            offset,
            buffer.len(),
        )?;

        let remaining = buffer.len() - offset - WAL_HEADER_SIZE;
        if !header.is_length_valid(remaining) {
            result.corrupted_count += 1;
            break;
        }

        let payload_start = offset + WAL_HEADER_SIZE;
        let payload_end = payload_start + header.length() as usize;

        if let Err(error) = WalOpType::try_from(header.op_type) {
            match recovery_mode {
                WalRecoveryMode::AbortOnCorruption => return Err(error),
                _ => {
                    result.corrupted_count += 1;
                    offset = payload_end;
                    continue;
                }
            }
        }

        let payload = buffer[payload_start..payload_end].to_vec();

        if let Some(computed) = checksum_mismatch(&header, &payload, verify_checksum) {
            match recovery_mode {
                WalRecoveryMode::AbortOnCorruption => {
                    return Err(WalError::ChecksumMismatch {
                        expected: header.checksum,
                        actual: computed,
                    });
                }
                _ => {
                    result.corrupted_count += 1;
                    offset = payload_end;
                    continue;
                }
            }
        }

        let final_payload = match decode_record_payload(&header, payload) {
            Ok(decompressed) => decompressed,
            Err(e) => match recovery_mode {
                WalRecoveryMode::AbortOnCorruption => return Err(e),
                _ => {
                    result.corrupted_count += 1;
                    offset = payload_end;
                    continue;
                }
            },
        };

        let record_type = header.record_type;
        let entry_lsn = header.lsn();
        expected_prev_lsn = entry_lsn;

        if record_type == RecordType::Full {
            push_entry(
                &mut result,
                ParsedWalEntry {
                    header,
                    payload: final_payload,
                    checksum_valid: true,
                    offset,
                    lsn: entry_lsn,
                    prev_lsn: header.prev_lsn(),
                    file_start_lsn,
                },
            );
        } else {
            let is_complete = fragment_buffer.add_fragment(header, final_payload);
            if is_complete {
                let assembled = fragment_buffer.assemble().unwrap_or_default();
                let first_header = fragment_buffer
                    .get_first_header()
                    .cloned()
                    .unwrap_or(header);
                fragment_buffer.reset();

                push_entry(
                    &mut result,
                    ParsedWalEntry {
                        header: first_header,
                        payload: assembled,
                        checksum_valid: true,
                        offset,
                        lsn: first_header.lsn(),
                        prev_lsn: first_header.prev_lsn(),
                        file_start_lsn,
                    },
                );
            }
        }

        offset = payload_end;
    }

    Ok(result)
}

/// A zero timestamp, zero length and zero LSN marks the zero-filled tail.
fn is_zero_padding(header: &WalHeader) -> bool {
    header.timestamp == 0 && header.length() == 0 && header.lsn == 0
}

/// Verify `header` continues the LSN chain from `expected_prev_lsn`.
fn validate_lsn_chain(
    header: &WalHeader,
    expected_prev_lsn: Lsn,
    file_start_lsn: Lsn,
    offset: usize,
    buffer_len: usize,
) -> WalResult<()> {
    let expected_lsn = header
        .prev_lsn()
        .as_u64()
        .checked_add(WAL_HEADER_SIZE as u64)
        .and_then(|lsn| lsn.checked_add(header.length() as u64))
        .ok_or_else(|| WalError::Corrupted(format!("LSN overflow at offset {}", offset)))?;
    if header.prev_lsn() != expected_prev_lsn || header.lsn() != Lsn::new(expected_lsn) {
        log::error!(
            "WAL LSN CHAIN DEBUG: file_start_lsn={}, expected_prev={}, got_prev={}, got_lsn={}, offset={}, buffer_len={}",
            file_start_lsn, expected_prev_lsn, header.prev_lsn(), header.lsn(), offset, buffer_len
        );
        return Err(WalError::Corrupted(format!(
            "Invalid LSN chain at offset {}: expected prev {}, got prev {}, lsn {}",
            offset,
            expected_prev_lsn,
            header.prev_lsn(),
            header.lsn()
        )));
    }
    Ok(())
}

/// `Some(computed)` when the stored checksum disagrees with the payload.
fn checksum_mismatch(header: &WalHeader, payload: &[u8], verify_checksum: bool) -> Option<u32> {
    if verify_checksum && header.checksum != 0 {
        let computed = compute_checksum(header, payload);
        if computed != header.checksum {
            return Some(computed);
        }
    }
    None
}

fn decode_record_payload(header: &WalHeader, payload: Vec<u8>) -> WalResult<Vec<u8>> {
    if header.is_compressed() {
        decompress_payload(&payload, header.compression())
    } else {
        Ok(payload)
    }
}

/// Append an entry and fold its timestamp/LSN into the running watermarks.
fn push_entry(result: &mut RecoveryResult, entry: ParsedWalEntry) {
    let timestamp = entry.header.timestamp;
    let lsn = entry.lsn;
    result.all_entries.push(entry);
    result.last_timestamp = result.last_timestamp.max(timestamp);
    if lsn > result.last_lsn {
        result.last_lsn = lsn;
    }
}

fn compute_checksum(header: &WalHeader, payload: &[u8]) -> u32 {
    use crc32fast::Hasher;
    let mut hasher = Hasher::new();
    hasher.update(&header.length.to_le_bytes());
    hasher.update(&[
        header.op_type,
        header.is_update as u8,
        header.record_type as u8,
    ]);
    hasher.update(&header.flags.to_le_bytes());
    hasher.update(&header.timestamp.to_le_bytes());
    hasher.update(&header.lsn.to_le_bytes());
    hasher.update(&header.prev_lsn.to_le_bytes());
    hasher.update(payload);
    hasher.finalize()
}

pub fn compute_checksum_public(header: &WalHeader, payload: &[u8]) -> u32 {
    compute_checksum(header, payload)
}

pub fn verify_entry_checksum(entry: &ParsedWalEntry) -> bool {
    if entry.header.checksum == 0 {
        return true;
    }
    compute_checksum(&entry.header, &entry.payload) == entry.header.checksum
}

fn decompress_payload(payload: &[u8], compression: WalCompression) -> WalResult<Vec<u8>> {
    match compression {
        WalCompression::Zstd => {
            zstd::decode_all(payload).map_err(|e| WalError::DeserializationError(e.to_string()))
        }
        WalCompression::None => Ok(payload.to_vec()),
    }
}

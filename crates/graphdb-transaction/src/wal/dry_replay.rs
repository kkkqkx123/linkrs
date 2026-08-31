//! Dry replay for WAL recovery
//!
//! Implements a Ladybug-style `dryReplay` that scans the WAL without applying
//! mutations, locating the last consistent commit boundary. This is used to
//! handle torn writes and partial WAL corruption at the tail — the common
//! crash scenario where the final transaction was not fully persisted.
//!
//! The algorithm walks the parsed entry stream, tracking the last offset that
//! corresponds to a completed `TransactionCommit` record. Any trailing entries
//! that lack a terminating `TransactionCommit` (or that fail checksum /
//! deserialization) are considered a torn tail and excluded from replay.

use std::path::Path;

use graphdb_core::types::Timestamp;
use graphdb_core::wal::types::{Lsn, WalConfig, WalError, WalOpType, WalRecoveryMode, WalResult};

use super::parser::{LocalWalParser, ParsedWalEntry, RecoveryResult, WalParser};

/// Result of a dry replay scan.
#[derive(Debug, Clone, Default)]
pub struct DryReplayResult {
    /// All entries up to and including the last consistent commit.
    /// Torn-tail entries after the last commit are excluded.
    pub consistent_entries: Vec<ParsedWalEntry>,
    /// LSN of the last consistent commit record (`Lsn::ZERO` if none).
    pub last_consistent_lsn: Lsn,
    /// Timestamp of the last consistent commit (`0` if none).
    pub last_consistent_timestamp: Timestamp,
    /// Whether the last record in the consistent prefix is a checkpoint marker.
    pub is_last_record_checkpoint: bool,
    /// Number of torn-tail entries discarded.
    pub truncated_tail: usize,
    /// Number of corrupted entries encountered during scan.
    pub corrupted_count: usize,
    /// Total entries scanned before truncation.
    pub total_scanned: usize,
}

/// Statistics emitted during dry replay (mirrors `RecoveryResult`).
#[derive(Debug, Clone, Default)]
pub struct DryReplayStats {
    pub total_entries: usize,
    pub consistent_entries: usize,
    pub truncated_tail: usize,
    pub corrupted_count: usize,
    pub last_consistent_lsn: Lsn,
    pub last_consistent_timestamp: Timestamp,
}

/// Perform a dry replay over WAL files in `wal_dir`.
///
/// This is the primary recovery entry point: it parses all WAL files, finds
/// the last consistent commit, and returns the truncated consistent prefix.
/// Callers can then replay only the consistent prefix via `RecoveryManager`.
///
/// `verify_checksum` controls per-entry CRC32 verification. When true,
/// corrupted checksums cause the tail to be truncated. When false, checksums
/// are ignored (useful for diagnostics).
///
/// `throw_on_corruption` mirrors Ladybug's `throwOnWalReplayFailure` flag:
/// when true, any corruption (including torn tails with checksum mismatch)
/// returns an error instead of truncating.
pub fn dry_replay(
    wal_dir: &Path,
    verify_checksum: bool,
    throw_on_corruption: bool,
) -> WalResult<DryReplayResult> {
    let recovery_mode = if throw_on_corruption {
        WalRecoveryMode::AbortOnCorruption
    } else {
        WalRecoveryMode::SkipCorruption
    };

    let parser_result = parse_wal_for_dry_replay(wal_dir, recovery_mode, verify_checksum)?;
    Ok(find_last_consistent_commit(
        parser_result,
        throw_on_corruption,
    ))
}

fn parse_wal_for_dry_replay(
    wal_dir: &Path,
    recovery_mode: WalRecoveryMode,
    verify_checksum: bool,
) -> WalResult<RecoveryResult> {
    if !wal_dir.exists() {
        return Ok(RecoveryResult::default());
    }
    let mut parser = LocalWalParser::new().with_verify_checksum(verify_checksum);
    parser = match recovery_mode {
        WalRecoveryMode::AbortOnCorruption => parser,
        _ => {
            LocalWalParser::with_recovery_mode(recovery_mode).with_verify_checksum(verify_checksum)
        }
    };
    parser
        .open(&wal_dir.to_string_lossy())
        .map_err(|e| match recovery_mode {
            WalRecoveryMode::AbortOnCorruption => e,
            _ => e,
        })?;

    Ok(RecoveryResult {
        all_entries: parser.parse_all_entries(),
        last_timestamp: parser.last_timestamp(),
        last_lsn: parser.last_lsn(),
        corrupted_count: parser.corrupted_count(),
        skipped_count: parser.skipped_count(),
    })
}

/// Scan parsed entries and return the prefix up to the last `TransactionCommit`.
/// Entries after the last commit are treated as a torn tail and discarded.
/// This faithfully reproduces ladybug's `WALReplayer::dryReplay` which tracks
/// `offsetDeserialized` at each `COMMIT_RECORD` and ignores trailing partial
/// records.
pub fn find_last_consistent_commit(
    mut result: RecoveryResult,
    throw_on_corruption: bool,
) -> DryReplayResult {
    if result.all_entries.is_empty() {
        return DryReplayResult {
            corrupted_count: result.corrupted_count,
            total_scanned: 0,
            ..Default::default()
        };
    }

    let total = result.all_entries.len();
    let corrupted = result.corrupted_count;

    if throw_on_corruption && corrupted > 0 {
        return DryReplayResult {
            total_scanned: total,
            corrupted_count: corrupted,
            ..Default::default()
        };
    }

    let has_sync_envelope = result.all_entries.iter().any(|e| {
        matches!(
            WalOpType::try_from(e.header.op_type),
            Ok(WalOpType::OutboxIntent
                | WalOpType::TransactionCommit
                | WalOpType::TransactionAbort)
        )
    });

    // Legacy WAL without sync envelope: all parsed entries are consistent.
    // Truncation only applies to torn-tail bytes already excluded by the
    // parser (reflected in corrupted_count). Return the full prefix.
    if !has_sync_envelope {
        let last_lsn = result.last_lsn;
        let last_ts = result.last_timestamp;
        let entries = result.all_entries;
        let is_last_checkpoint = entries
            .last()
            .and_then(|e| WalOpType::try_from(e.header.op_type).ok())
            .map(|op| op == WalOpType::Compact)
            .unwrap_or(false);
        return DryReplayResult {
            consistent_entries: entries,
            last_consistent_lsn: last_lsn,
            last_consistent_timestamp: last_ts,
            is_last_record_checkpoint: is_last_checkpoint,
            truncated_tail: 0,
            corrupted_count: corrupted,
            total_scanned: total,
        };
    }

    // Collect committed transaction boundaries using the canonical commit
    // collector — only transactions terminated by a TransactionCommit record
    // are considered consistent.
    let committed = match super::commit::collect_committed_transactions(&result.all_entries) {
        Ok(c) => c,
        Err(_) => {
            // Checksum or batch mismatch: treat the entire stream up to the
            // last commit as corrupted. Fall back to scanning for commit
            // records directly, truncating the uncommitted tail.
            return scan_for_last_commit_fallback(result);
        }
    };

    if committed.is_empty() {
        // Has sync envelope but no complete commit — the WAL tail is uncommitted.
        // This can happen if the last transaction's redo entries were persisted
        // but the final TransactionCommit record was torn. Treat the tail as
        // truncated and return empty consistent prefix (unless throw_on_corruption).
        if !throw_on_corruption {
            // Check if there was at least one commit marker; if not, the whole
            // envelope-bearing stream is torn and we return empty.
            let has_commit = result.all_entries.iter().any(|e| {
                matches!(
                    WalOpType::try_from(e.header.op_type),
                    Ok(WalOpType::TransactionCommit)
                )
            });
            if !has_commit {
                // All redo entries without a commit marker are torn tail.
                return DryReplayResult {
                    consistent_entries: Vec::new(),
                    last_consistent_lsn: Lsn::ZERO,
                    last_consistent_timestamp: 0,
                    is_last_record_checkpoint: false,
                    truncated_tail: total,
                    corrupted_count: corrupted,
                    total_scanned: total,
                };
            }
        }
        // Fallback path for mixed cases
        return scan_for_last_commit_fallback(result);
    }

    // The consistent prefix is all entries up to and including the last
    // committed transaction's commit LSN. Anything after that LSN is torn.
    let last_commit_lsn = committed
        .last()
        .map(|c| Lsn::new(c.commit_lsn.get()))
        .unwrap_or(Lsn::ZERO);
    let last_ts = committed
        .last()
        .and_then(|c| c.redo_entries.first().map(|e| e.header.timestamp))
        .unwrap_or(0);

    // Determine cutoff index: include entries up to commit_lsn inclusive.
    let cutoff_idx = result
        .all_entries
        .iter()
        .position(|e| e.lsn == last_commit_lsn)
        .map(|pos| pos + 1)
        .unwrap_or(result.all_entries.len());

    let truncated = total.saturating_sub(cutoff_idx);
    let total_scanned = total;
    let entries_truncated = if truncated > 0 {
        result.all_entries.truncate(cutoff_idx);
        true
    } else {
        false
    };
    let _ = entries_truncated;

    // Detect checkpoint marker as last record (Compact op is used as a loose
    // checkpoint proxy until a dedicated checkpoint record type is added).
    let is_last_checkpoint = result
        .all_entries
        .last()
        .and_then(|e| WalOpType::try_from(e.header.op_type).ok())
        .map(|op| op == WalOpType::Compact)
        .unwrap_or(false);

    DryReplayResult {
        consistent_entries: result.all_entries,
        last_consistent_lsn: last_commit_lsn,
        last_consistent_timestamp: last_ts,
        is_last_record_checkpoint: is_last_checkpoint,
        truncated_tail: truncated,
        corrupted_count: corrupted,
        total_scanned,
    }
}

fn scan_for_last_commit_fallback(mut result: RecoveryResult) -> DryReplayResult {
    let total = result.all_entries.len();
    // Find last TransactionCommit in the entry stream.
    let last_commit_pos = result.all_entries.iter().rposition(|e| {
        WalOpType::try_from(e.header.op_type)
            .map(|op| op == WalOpType::TransactionCommit)
            .unwrap_or(false)
    });
    match last_commit_pos {
        Some(pos) => {
            let commit_lsn = result.all_entries[pos].lsn;
            let commit_ts = result.all_entries[pos].header.timestamp;
            let truncated = total.saturating_sub(pos + 1);
            result.all_entries.truncate(pos + 1);
            let is_last_checkpoint =
                WalOpType::try_from(result.all_entries.last().unwrap().header.op_type)
                    .map(|op| op == WalOpType::Compact)
                    .unwrap_or(false);
            DryReplayResult {
                consistent_entries: result.all_entries,
                last_consistent_lsn: commit_lsn,
                last_consistent_timestamp: commit_ts,
                is_last_record_checkpoint: is_last_checkpoint,
                truncated_tail: truncated,
                corrupted_count: result.corrupted_count,
                total_scanned: total,
            }
        }
        None => DryReplayResult {
            consistent_entries: Vec::new(),
            last_consistent_lsn: Lsn::ZERO,
            last_consistent_timestamp: 0,
            is_last_record_checkpoint: false,
            truncated_tail: total,
            corrupted_count: result.corrupted_count,
            total_scanned: total,
        },
    }
}

/// Verify checksums for all entries in the parsed result, returning an error
/// if any entry fails when `strict` is true. When `strict` is false, the
/// corrupted entries are counted and reported in the result.
pub fn verify_checksums(result: &RecoveryResult, strict: bool) -> WalResult<usize> {
    let mut corrupted = 0usize;
    for entry in &result.all_entries {
        if entry.header.checksum != 0 {
            let computed =
                crate::wal::parser::compute_checksum_public(&entry.header, &entry.payload);
            if computed != entry.header.checksum {
                corrupted += 1;
                if strict {
                    return Err(WalError::ChecksumMismatch {
                        expected: entry.header.checksum,
                        actual: computed,
                    });
                }
            }
        }
    }
    Ok(corrupted)
}

/// Configuration for dry replay used by higher-level recovery.
#[derive(Debug, Clone)]
pub struct DryReplayConfig {
    pub verify_checksum: bool,
    pub throw_on_corruption: bool,
    pub wal_config: WalConfig,
}

impl Default for DryReplayConfig {
    fn default() -> Self {
        Self {
            verify_checksum: true,
            throw_on_corruption: false,
            wal_config: WalConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::{
        LocalWalParser, LocalWalWriter, TransactionWalEntry, WalConfig, WalOpType, WalParser,
    };
    use graphdb_core::types::{
        IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
    };
    use graphdb_core::wal::traits::WalWriter;
    use graphdb_core::wal::{
        EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION,
    };
    use tempfile::TempDir;

    fn make_intent(tx: u64, seq: u32) -> OutboxIntent {
        OutboxIntent {
            wire_version: WAL_SYNC_WIRE_VERSION,
            transaction_id: TransactionId::new(tx),
            intent_sequence: seq,
            mutation: IndexMutation {
                wire_version: WAL_SYNC_WIRE_VERSION,
                target: TargetId::new("fulltext").unwrap(),
                index_id: 1,
                index_generation: IndexGeneration::new(1),
                entity_ref: EntityRef::Vertex(VertexId::from_int64(1)),
                operation: IndexOperation::Upsert,
                document_or_vector: vec![1],
                idempotency_key: IdempotencyKey::new(format!("txn-{}:{}", tx, seq)).unwrap(),
                ordering_key: OrderingKey::new(format!("k-{}-{}", tx, seq)).unwrap(),
            },
        }
    }

    #[test]
    fn dry_replay_finds_last_consistent_commit() {
        let dir = TempDir::new().unwrap();
        let wal_path = dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().unwrap();
        let tx1 = TransactionId::new(1);
        writer
            .append_transaction_batch(
                tx1,
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 1,
                    payload: b"a".to_vec(),
                }],
                &[make_intent(1, 0)],
            )
            .unwrap();
        let tx2 = TransactionId::new(2);
        writer
            .append_transaction_batch(
                tx2,
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 2,
                    payload: b"b".to_vec(),
                }],
                &[make_intent(2, 0)],
            )
            .unwrap();
        writer.close();

        let result = dry_replay(dir.path(), true, false).unwrap();
        assert_eq!(result.consistent_entries.len(), 6);
        assert!(result.last_consistent_lsn > Lsn::ZERO);
        assert_eq!(result.truncated_tail, 0);
        assert_eq!(result.corrupted_count, 0);
    }

    #[test]
    fn dry_replay_truncates_torn_tail() {
        let dir = TempDir::new().unwrap();
        let wal_path = dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().unwrap();
        let tx1 = TransactionId::new(10);
        writer
            .append_transaction_batch(
                tx1,
                vec![TransactionWalEntry {
                    op_type: WalOpType::InsertVertex,
                    timestamp: 1,
                    payload: b"committed".to_vec(),
                }],
                &[],
            )
            .unwrap();
        let lsn_after_commit = writer.current_lsn();
        writer
            .append_entry(WalOpType::InsertVertex, 2, b"torn_tail")
            .unwrap();
        writer.sync().unwrap();
        writer.close();

        let mut parser = LocalWalParser::new();
        parser.open(&wal_path).unwrap();
        let total = parser.parse_all_entries().len();
        assert_eq!(total, 3);

        let dry = dry_replay(dir.path(), true, false).unwrap();
        assert_eq!(dry.consistent_entries.len(), 2);
        assert_eq!(dry.truncated_tail, 1);
        assert_eq!(dry.last_consistent_lsn, Lsn::new(lsn_after_commit.as_u64()));
    }

    #[test]
    fn dry_replay_empty_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let result = dry_replay(dir.path(), true, false).unwrap();
        assert!(result.consistent_entries.is_empty());
        assert_eq!(result.last_consistent_lsn, Lsn::ZERO);
        assert_eq!(result.truncated_tail, 0);
    }

    #[test]
    fn dry_replay_checksum_corrupted_tail_is_truncated() {
        use graphdb_core::wal::types::{WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE};
        use std::fs::OpenOptions;
        use std::io::{Seek, SeekFrom, Write};

        let dir = TempDir::new().unwrap();
        let wal_path = dir.path().to_string_lossy().to_string();
        let mut writer =
            LocalWalWriter::with_config(&wal_path, 0, WalConfig::new().with_checksum(true));
        writer.open().unwrap();
        writer
            .append_entry(WalOpType::InsertVertex, 1, b"good")
            .unwrap();
        writer.sync().unwrap();
        let used = writer.file_used();
        writer.close();

        let wal_file = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("_wal_"))
                    .unwrap_or(false)
            })
            .unwrap();
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&wal_file)
            .unwrap();
        f.seek(SeekFrom::Start(
            (WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE) as u64,
        ))
        .unwrap();
        let mut byte = [0u8; 1];
        use std::io::Read;
        f.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xFF;
        f.seek(SeekFrom::Start(
            (WAL_FILE_HEADER_SIZE + WAL_HEADER_SIZE) as u64,
        ))
        .unwrap();
        f.write_all(&byte).unwrap();
        f.sync_all().unwrap();
        drop(f);
        let _ = used;

        let dry = dry_replay(dir.path(), true, false).unwrap();
        assert!(
            dry.corrupted_count > 0 || dry.truncated_tail > 0 || dry.consistent_entries.is_empty()
        );
    }
}

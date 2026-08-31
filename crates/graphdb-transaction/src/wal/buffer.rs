//! Two-Tier WAL: Local Buffer per Transaction
//!
//! Provides an in-memory WAL buffer that holds redo records for a single
//! transaction until commit time, mirroring Ladybug's `LocalWAL` design.
//! Entries are appended to the buffer during execution without touching the
//! global WAL or requiring an fsync. At commit the buffer is atomically flushed
//! to the global `LocalWalWriter`, optionally via group-commit batching.

use std::sync::atomic::{AtomicUsize, Ordering};

use graphdb_core::types::{CommitLsn, TransactionId};
use graphdb_core::wal::types::{WalConfig, WalError, WalOpType, WalResult};
use graphdb_core::wal::{OutboxIntent, WAL_SYNC_WIRE_VERSION};

use super::commit::TransactionWalEntry;
use crate::wal::LocalWalWriter;

/// Configuration for the per-transaction local WAL buffer.
#[derive(Debug, Clone)]
pub struct LocalWalBufferConfig {
    /// Maximum bytes to buffer locally before a warning is emitted.
    /// 0 = unlimited.
    pub max_buffer_bytes: usize,
    /// Whether to validate checksums on buffered entries.
    pub checksum_enabled: bool,
    /// Group commit enabled for the flush path.
    pub group_commit_enabled: bool,
}

impl Default for LocalWalBufferConfig {
    fn default() -> Self {
        Self {
            max_buffer_bytes: 16 * 1024 * 1024,
            checksum_enabled: true,
            group_commit_enabled: true,
        }
    }
}

/// In-memory per-transaction WAL buffer.
///
/// Buffers `TransactionWalEntry` records and `OutboxIntent` records without
/// any disk I/O. The buffer is flushed once at commit time into the shared
/// global WAL via `LocalWalWriter::append_transaction_batch` or the
/// non-blocking `append_transaction_batch_no_wait` path that integrates with
/// the `GroupCommitCoordinator` for batch fsync sharing.
#[derive(Debug, Default)]
pub struct LocalWalBuffer {
    entries: Vec<TransactionWalEntry>,
    intents: Vec<OutboxIntent>,
    buffered_bytes: AtomicUsize,
    config: LocalWalBufferConfig,
}

impl LocalWalBuffer {
    pub fn new() -> Self {
        Self::with_config(LocalWalBufferConfig::default())
    }

    pub fn with_config(config: LocalWalBufferConfig) -> Self {
        Self {
            entries: Vec::new(),
            intents: Vec::new(),
            buffered_bytes: AtomicUsize::new(0),
            config,
        }
    }

    /// Append a redo entry to the local buffer (no disk I/O).
    pub fn append_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: u64,
        payload: Vec<u8>,
    ) -> WalResult<()> {
        let entry_len = payload.len() + std::mem::size_of::<WalOpType>() + 8;
        let new_total = self.buffered_bytes.load(Ordering::Relaxed) + entry_len;
        if self.config.max_buffer_bytes != 0 && new_total > self.config.max_buffer_bytes {
            log::warn!(
                "LocalWAL buffer exceeds limit: {} > {} (entries={}, intents={})",
                new_total,
                self.config.max_buffer_bytes,
                self.entries.len(),
                self.intents.len()
            );
        }
        self.entries.push(TransactionWalEntry {
            op_type,
            timestamp,
            payload,
        });
        self.buffered_bytes.store(new_total, Ordering::Relaxed);
        Ok(())
    }

    /// Append an outbox intent to the local buffer.
    pub fn append_intent(&mut self, intent: OutboxIntent) -> WalResult<()> {
        intent.validate().map_err(WalError::InvalidOperation)?;
        let intent_len = std::mem::size_of_val(&intent.intent_sequence)
            + intent.mutation.document_or_vector.len()
            + 64;
        let new_total = self.buffered_bytes.load(Ordering::Relaxed) + intent_len;
        if self.config.max_buffer_bytes != 0 && new_total > self.config.max_buffer_bytes {
            log::warn!(
                "LocalWAL buffer exceeds limit after intent: {} > {}",
                new_total,
                self.config.max_buffer_bytes
            );
        }
        self.intents.push(intent);
        self.buffered_bytes.store(new_total, Ordering::Relaxed);
        Ok(())
    }

    /// Number of buffered redo entries.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Number of buffered intents.
    pub fn intent_count(&self) -> usize {
        self.intents.len()
    }

    /// Total buffered bytes estimate.
    pub fn buffered_bytes(&self) -> usize {
        self.buffered_bytes.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.intents.is_empty()
    }

    /// Borrow buffered entries for inspection.
    pub fn entries(&self) -> &[TransactionWalEntry] {
        &self.entries
    }

    /// Borrow buffered intents for inspection.
    pub fn intents(&self) -> &[OutboxIntent] {
        &self.intents
    }

    /// Clear the buffer without flushing (used on abort / savepoint rollback).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.intents.clear();
        self.buffered_bytes.store(0, Ordering::Relaxed);
    }

    /// Truncate to a previous length (savepoint rollback support).
    pub fn truncate(&mut self, entry_len: usize, intent_len: usize) {
        self.entries.truncate(entry_len);
        self.intents.truncate(intent_len);
        let bytes: usize = self
            .entries
            .iter()
            .map(|e| e.payload.len() + 16)
            .sum::<usize>()
            + self
                .intents
                .iter()
                .map(|i| i.mutation.document_or_vector.len() + 64)
                .sum::<usize>();
        self.buffered_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Flush the buffered entries to the global WAL atomically.
    ///
    /// This consumes the buffer contents and writes them as a single
    /// transactional batch to `writer`. The caller is responsible for
    /// deciding durability (Sync vs Async) and, when using group-commit,
    /// for batching fsync via `GroupCommitCoordinator`.
    pub fn flush_to_writer(
        &mut self,
        writer: &mut LocalWalWriter,
        transaction_id: TransactionId,
        durability: graphdb_core::types::DurabilityLevel,
    ) -> WalResult<CommitLsn> {
        if self.is_empty() {
            return Ok(CommitLsn::new(writer.current_lsn().as_u64()));
        }
        let entries = std::mem::take(&mut self.entries);
        let intents = std::mem::take(&mut self.intents);
        self.buffered_bytes.store(0, Ordering::Relaxed);
        let result = if durability == graphdb_core::types::DurabilityLevel::None {
            writer.append_transaction_batch_no_wait(transaction_id, entries, &intents)?;
            CommitLsn::new(writer.current_lsn().as_u64())
        } else {
            writer.append_transaction_batch_with_durability(
                transaction_id,
                entries,
                &intents,
                durability,
            )?
        };
        Ok(result)
    }

    /// Flush without waiting for durability, returning the commit LSN.
    ///
    /// The caller must arrange durability after releasing any writer lock
    /// (e.g. via `GroupCommitCoordinator::append_and_wait`). This is the
    /// preferred path for high-concurrency commit batching.
    pub fn flush_no_wait(
        &mut self,
        writer: &mut LocalWalWriter,
        transaction_id: TransactionId,
    ) -> WalResult<CommitLsn> {
        if self.is_empty() {
            return Ok(CommitLsn::new(writer.current_lsn().as_u64()));
        }
        let entries = std::mem::take(&mut self.entries);
        let intents = std::mem::take(&mut self.intents);
        self.buffered_bytes.store(0, Ordering::Relaxed);
        writer.append_transaction_batch_no_wait(transaction_id, entries, &intents)
    }

    /// Build transaction entries for inspection without flushing (e.g. for
    /// commit path that delegates WAL writing to storage's commit sink).
    pub fn build_transaction_entries(
        &self,
        transaction_id: TransactionId,
    ) -> WalResult<Vec<TransactionWalEntry>> {
        let mut entries = self.entries.clone();
        for (seq, intent) in self.intents.iter().enumerate() {
            if intent.intent_sequence as usize != seq {
                return Err(WalError::InvalidOperation(format!(
                    "Intent sequence gap: expected {}, got {}",
                    seq, intent.intent_sequence
                )));
            }
            if intent.transaction_id != transaction_id {
                return Err(WalError::InvalidOperation(format!(
                    "Intent transaction mismatch: {} vs {}",
                    intent.transaction_id, transaction_id
                )));
            }
            entries.push(TransactionWalEntry {
                op_type: WalOpType::OutboxIntent,
                timestamp: 0,
                payload: postcard::to_allocvec(intent)?,
            });
        }
        if !entries.is_empty() || !self.intents.is_empty() {
            let batch_checksum = super::commit::batch_checksum(&entries);
            let commit = graphdb_core::wal::TransactionCommit {
                wire_version: WAL_SYNC_WIRE_VERSION,
                transaction_id,
                intent_count: self.intents.len() as u32,
                batch_checksum,
            };
            entries.push(TransactionWalEntry {
                op_type: WalOpType::TransactionCommit,
                timestamp: 0,
                payload: postcard::to_allocvec(&commit)?,
            });
        }
        Ok(entries)
    }

    /// Create a WAL config that reflects this buffer's settings.
    pub fn wal_config(&self) -> WalConfig {
        WalConfig::default()
            .with_checksum(self.config.checksum_enabled)
            .with_group_commit(self.config.group_commit_enabled)
    }

    /// Returns estimated memory usage for metrics.
    pub fn memory_usage(&self) -> usize {
        self.buffered_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wal::parser::WalParser;
    use graphdb_core::types::{IdempotencyKey, IndexGeneration, OrderingKey, TargetId, VertexId};
    use graphdb_core::wal::traits::WalWriter;
    use graphdb_core::wal::{EntityRef, IndexMutation, IndexOperation};

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
                document_or_vector: vec![1, 2, 3],
                idempotency_key: IdempotencyKey::new(format!("txn-{}:{}", tx, seq)).unwrap(),
                ordering_key: OrderingKey::new(format!("k-{}-{}", tx, seq)).unwrap(),
            },
        }
    }

    #[test]
    fn local_buffer_append_and_flush_via_writer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal_path = temp_dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().unwrap();

        let mut buffer = LocalWalBuffer::new();
        buffer
            .append_entry(WalOpType::InsertVertex, 1, b"hello".to_vec())
            .unwrap();
        buffer.append_intent(make_intent(42, 0)).unwrap();
        assert_eq!(buffer.entry_count(), 1);
        assert_eq!(buffer.intent_count(), 1);
        assert!(!buffer.is_empty());

        let c_lsn = buffer
            .flush_to_writer(
                &mut writer,
                TransactionId::new(42),
                graphdb_core::types::DurabilityLevel::Sync,
            )
            .unwrap();
        assert!(c_lsn.get() > 0);
        assert!(buffer.is_empty());

        writer.close();

        let mut parser = crate::wal::LocalWalParser::new();
        parser.open(&wal_path).unwrap();
        let committed =
            crate::wal::collect_committed_transactions(&parser.parse_all_entries()).unwrap();
        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].transaction_id, TransactionId::new(42));
        assert_eq!(committed[0].redo_entries.len(), 1);
        assert_eq!(committed[0].intents.len(), 1);
    }

    #[test]
    fn local_buffer_no_wait_and_group_commit_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal_path = temp_dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().unwrap();

        let mut buf = LocalWalBuffer::new();
        buf.append_entry(WalOpType::InsertVertex, 2, b"payload".to_vec())
            .unwrap();
        let lsn = buf
            .flush_no_wait(&mut writer, TransactionId::new(7))
            .unwrap();
        writer.sync().unwrap();
        assert!(lsn.get() > 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn local_buffer_truncate_restores_state() {
        let mut buf = LocalWalBuffer::new();
        buf.append_entry(WalOpType::InsertVertex, 1, b"a".to_vec())
            .unwrap();
        buf.append_entry(WalOpType::InsertEdge, 2, b"b".to_vec())
            .unwrap();
        buf.append_intent(make_intent(1, 0)).unwrap();
        assert_eq!(buf.entry_count(), 2);
        buf.truncate(1, 0);
        assert_eq!(buf.entry_count(), 1);
        assert_eq!(buf.intent_count(), 0);
    }

    #[test]
    fn local_buffer_build_entries_includes_commit() {
        let mut buf = LocalWalBuffer::new();
        buf.append_entry(WalOpType::InsertVertex, 10, b"data".to_vec())
            .unwrap();
        let entries = buf
            .build_transaction_entries(TransactionId::new(99))
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.last().unwrap().op_type,
            WalOpType::TransactionCommit
        );
    }

    #[test]
    fn empty_flush_returns_current_lsn() {
        let temp_dir = tempfile::tempdir().unwrap();
        let wal_path = temp_dir.path().to_string_lossy().to_string();
        let mut writer = LocalWalWriter::new(&wal_path, 0);
        writer.open().unwrap();
        let cur = writer.current_lsn();
        let mut buf = LocalWalBuffer::new();
        let lsn = buf
            .flush_to_writer(
                &mut writer,
                TransactionId::new(5),
                graphdb_core::types::DurabilityLevel::Sync,
            )
            .unwrap();
        assert_eq!(lsn, CommitLsn::new(cur.as_u64()));
    }
}

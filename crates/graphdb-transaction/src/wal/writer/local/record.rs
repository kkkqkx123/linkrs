//! Local WAL writer - record module

use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::wal::writer::compression::{self as compression_mod, create_compressor, Compressor};
use crate::wal::writer::sync::elapsed_since;
use graphdb_core::types::Timestamp;
use graphdb_core::wal::types::{
    Lsn, RecordType, WalCompression, WalError, WalHeader, WalOpType, WalResult, WalStats,
    WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE, WAL_MAX_RECORD_SIZE,
};

use super::{LocalWalWriter, WalHeaderParams};

impl LocalWalWriter {
    /// Append a WAL entry with checksum and LSN
    pub fn append_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
    ) -> WalResult<()> {
        self.check_poisoned()?;
        if !self.is_open.load(Ordering::SeqCst) {
            return Err(WalError::Closed);
        }

        let (final_payload, compression) = self.compressor.compress(payload)?;

        if final_payload.len() > WAL_MAX_RECORD_SIZE {
            return self.append_fragmented_entry(op_type, timestamp, &final_payload, compression);
        }

        self.append_single_entry(op_type, timestamp, &final_payload, compression)
    }

    /// Append a single (non-fragmented) WAL entry
    pub(crate) fn append_single_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
        compression: WalCompression,
    ) -> WalResult<()> {
        let prev_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));
        let entry_size = WAL_HEADER_SIZE + payload.len();
        let new_lsn = Lsn::new(prev_lsn.as_u64() + entry_size as u64);

        let header = self.build_wal_header(WalHeaderParams {
            op_type,
            timestamp,
            payload_len: payload.len(),
            prev_lsn,
            new_lsn,
            record_type: RecordType::Full,
            payload,
            compression,
        });

        self.write_entry(&header, payload, new_lsn)
    }

    /// Append a fragmented WAL entry (for large payloads)
    pub(crate) fn append_fragmented_entry(
        &mut self,
        op_type: WalOpType,
        timestamp: Timestamp,
        payload: &[u8],
        compression: WalCompression,
    ) -> WalResult<()> {
        let total_chunks = payload.len().div_ceil(WAL_MAX_RECORD_SIZE);
        let mut offset = 0;
        let mut chunk_index = 0;
        let mut first_lsn = Lsn::ZERO;
        let mut chunks_written = 0;

        while offset < payload.len() {
            let chunk_end = (offset + WAL_MAX_RECORD_SIZE).min(payload.len());
            let chunk_data = &payload[offset..chunk_end];
            let chunk_size = chunk_data.len();

            let prev_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));
            let entry_size = WAL_HEADER_SIZE + chunk_size;
            let new_lsn = Lsn::new(prev_lsn.as_u64() + entry_size as u64);

            if chunk_index == 0 {
                first_lsn = new_lsn;
            }

            let record_type = if total_chunks == 1 {
                RecordType::Full
            } else if chunk_index == 0 {
                RecordType::First
            } else if chunk_index == total_chunks - 1 {
                RecordType::Last
            } else {
                RecordType::Middle
            };

            let header = self.build_wal_header(WalHeaderParams {
                op_type,
                timestamp,
                payload_len: chunk_size,
                prev_lsn,
                new_lsn,
                record_type,
                payload: chunk_data,
                compression,
            });

            if let Err(e) = self.write_entry(&header, chunk_data, new_lsn) {
                log::error!(
                    "Failed to write chunk {}/{} of fragmented WAL entry (first_lsn: {}, written: {}): {}",
                    chunk_index + 1,
                    total_chunks,
                    first_lsn.as_u64(),
                    chunks_written,
                    e
                );
                return Err(e);
            }

            offset = chunk_end;
            chunk_index += 1;
            chunks_written += 1;
        }

        Ok(())
    }

    /// Write a single entry to the file
    pub(crate) fn write_entry(
        &mut self,
        header: &WalHeader,
        payload: &[u8],
        new_lsn: Lsn,
    ) -> WalResult<()> {
        let header_bytes = header.as_bytes();

        let file = self.file.as_mut().ok_or(WalError::Closed)?;
        let total_len = header_bytes.len() + payload.len();

        let expected_size = self.file_used + total_len;
        if expected_size > self.file_size {
            let new_size =
                ((expected_size / self.config.truncate_size) + 1) * self.config.truncate_size;
            file.set_len(new_size as u64)?;
            self.file_size = new_size;
        }

        file.seek(SeekFrom::Start(self.file_used as u64))?;
        file.write_all(&header_bytes)?;
        file.write_all(payload)?;
        self.file_used += total_len;

        self.current_lsn.store(new_lsn.as_u64(), Ordering::SeqCst);

        let write_count = self.write_count.fetch_add(1, Ordering::SeqCst) + 1;
        let elapsed = elapsed_since(*self.last_sync_time.lock().unwrap());
        if self.config.sync_policy.requires_sync(write_count, elapsed) {
            if let Err(e) = file.sync_data() {
                self.poison(format!("fsync failed: {}", e));
                return Err(WalError::IoError(e.to_string()));
            }
            let lsn = self.current_lsn.load(Ordering::SeqCst);
            self.last_synced_lsn.store(lsn, Ordering::SeqCst);
            self.write_count.store(0, Ordering::SeqCst);
            if let Ok(mut guard) = self.last_sync_time.lock() {
                *guard = Some(Instant::now());
            }
        }

        Ok(())
    }

    pub(crate) fn build_wal_header(&self, params: WalHeaderParams<'_>) -> WalHeader {
        let header = WalHeader::new(params.op_type, params.timestamp, params.payload_len as u32)
            .with_lsn(params.new_lsn, params.prev_lsn)
            .with_record_type(params.record_type)
            .with_compression(params.compression);
        if self.config.checksum_enabled {
            header.with_checksum(params.payload)
        } else {
            header
        }
    }

    /// Append multiple entries as a batch (for group commit)
    pub fn append_batch(&mut self, entries: &[(WalOpType, Timestamp, &[u8])]) -> WalResult<()> {
        self.append_batch_with_durability(entries, graphdb_core::types::DurabilityLevel::Sync)
    }

    pub fn append_batch_with_durability(
        &mut self,
        entries: &[(WalOpType, Timestamp, &[u8])],
        durability: graphdb_core::types::DurabilityLevel,
    ) -> WalResult<()> {
        self.check_poisoned()?;
        if !self.is_open.load(Ordering::SeqCst) {
            return Err(WalError::Closed);
        }

        let new_lsn = self.write_batch_entries(entries)?;

        if matches!(durability, graphdb_core::types::DurabilityLevel::Sync) {
            if let Some(ref coordinator) = self.group_commit {
                coordinator.record_appended(new_lsn);
                coordinator.append_and_wait(new_lsn)?;
            } else {
                let file = self.file.as_mut().ok_or(WalError::Closed)?;
                file.sync_data()?;
            }
            self.last_synced_lsn.store(new_lsn, Ordering::SeqCst);
        }

        Ok(())
    }

    /// Write a batch of entries to the WAL file and return the new LSN without
    /// waiting for durability.
    ///
    /// The caller is responsible for arranging durability (e.g. via
    /// [`wait_for_durable`](WalWriter::wait_for_durable) or the group-commit
    /// coordinator) **outside** any writer lock, so concurrent commits can
    /// share a single fsync.
    pub(crate) fn write_batch_entries(
        &mut self,
        entries: &[(WalOpType, Timestamp, &[u8])],
    ) -> WalResult<u64> {
        let mut total_len = 0;
        let mut compressed_entries = Vec::with_capacity(entries.len());

        for (op_type, timestamp, payload) in entries {
            let (final_payload, compression) = self.compressor.compress(payload)?;

            let prev_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst) + total_len as u64);
            let entry_size = WAL_HEADER_SIZE + final_payload.len();
            let new_lsn = Lsn::new(prev_lsn.as_u64() + entry_size as u64);

            let header = self.build_wal_header(WalHeaderParams {
                op_type: *op_type,
                timestamp: *timestamp,
                payload_len: final_payload.len(),
                prev_lsn,
                new_lsn,
                record_type: RecordType::Full,
                payload: &final_payload,
                compression,
            });

            total_len += WAL_HEADER_SIZE + final_payload.len();
            compressed_entries.push((header, final_payload));
        }

        let file = self.file.as_mut().ok_or(WalError::Closed)?;

        let expected_size = self.file_used + total_len;
        if expected_size > self.file_size {
            let new_size =
                ((expected_size / self.config.truncate_size) + 1) * self.config.truncate_size;
            file.set_len(new_size as u64)?;
            self.file_size = new_size;
        }

        file.seek(SeekFrom::Start(self.file_used as u64))?;

        for (header, payload) in compressed_entries {
            file.write_all(&header.as_bytes())?;
            file.write_all(&payload)?;
        }

        self.file_used += total_len;

        let new_lsn = self.current_lsn.load(Ordering::SeqCst) + total_len as u64;
        self.current_lsn.store(new_lsn, Ordering::SeqCst);

        Ok(new_lsn)
    }

    pub fn append_transaction_batch(
        &mut self,
        transaction_id: graphdb_core::types::TransactionId,
        entries: Vec<crate::wal::TransactionWalEntry>,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> WalResult<graphdb_core::types::CommitLsn> {
        self.append_transaction_batch_with_durability(
            transaction_id,
            entries,
            intents,
            graphdb_core::types::DurabilityLevel::Sync,
        )
    }

    pub fn append_transaction_batch_with_durability(
        &mut self,
        transaction_id: graphdb_core::types::TransactionId,
        entries: Vec<crate::wal::TransactionWalEntry>,
        intents: &[graphdb_core::wal::OutboxIntent],
        durability: graphdb_core::types::DurabilityLevel,
    ) -> WalResult<graphdb_core::types::CommitLsn> {
        let entries = self.build_transaction_entries(transaction_id, entries, intents)?;
        let entry_refs = entries
            .iter()
            .map(|entry| (entry.op_type, entry.timestamp, entry.payload.as_slice()))
            .collect::<Vec<_>>();
        self.append_batch_with_durability(&entry_refs, durability)?;
        Ok(graphdb_core::types::CommitLsn::new(
            self.current_lsn().as_u64(),
        ))
    }

    /// Append a committed transaction to the WAL **without** waiting for
    /// durability, returning the commit LSN.
    ///
    /// The caller must arrange durability after releasing any writer lock
    /// (e.g. via the group-commit coordinator) so concurrent commits batch
    /// into a single fsync.
    pub fn append_transaction_batch_no_wait(
        &mut self,
        transaction_id: graphdb_core::types::TransactionId,
        entries: Vec<crate::wal::TransactionWalEntry>,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> WalResult<graphdb_core::types::CommitLsn> {
        let entries = self.build_transaction_entries(transaction_id, entries, intents)?;
        let entry_refs = entries
            .iter()
            .map(|entry| (entry.op_type, entry.timestamp, entry.payload.as_slice()))
            .collect::<Vec<_>>();
        self.write_batch_entries(&entry_refs)?;
        Ok(graphdb_core::types::CommitLsn::new(
            self.current_lsn().as_u64(),
        ))
    }

    /// Build the full entry list for a committed transaction (intent records
    /// plus the commit record).
    pub(crate) fn build_transaction_entries(
        &self,
        transaction_id: graphdb_core::types::TransactionId,
        mut entries: Vec<crate::wal::TransactionWalEntry>,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> WalResult<Vec<crate::wal::TransactionWalEntry>> {
        self.check_poisoned()?;
        for (expected, intent) in intents.iter().enumerate() {
            intent.validate().map_err(WalError::InvalidOperation)?;
            if intent.transaction_id != transaction_id {
                return Err(WalError::InvalidOperation(format!(
                    "Intent transaction {} does not match batch transaction {}",
                    intent.transaction_id, transaction_id
                )));
            }
            if intent.intent_sequence as usize != expected {
                return Err(WalError::InvalidOperation(format!(
                    "Intent sequence is not contiguous: expected {}, got {}",
                    expected, intent.intent_sequence
                )));
            }
            entries.push(crate::wal::TransactionWalEntry {
                op_type: WalOpType::OutboxIntent,
                timestamp: 0,
                payload: postcard::to_allocvec(intent)?,
            });
        }
        let commit = graphdb_core::wal::TransactionCommit {
            wire_version: graphdb_core::wal::WAL_SYNC_WIRE_VERSION,
            transaction_id,
            intent_count: u32::try_from(intents.len()).map_err(|_| {
                WalError::InvalidOperation("Intent count exceeds u32 range".to_string())
            })?,
            batch_checksum: crate::wal::commit::batch_checksum(&entries),
        };
        entries.push(crate::wal::TransactionWalEntry {
            op_type: WalOpType::TransactionCommit,
            timestamp: 0,
            payload: postcard::to_allocvec(&commit)?,
        });
        Ok(entries)
    }

    /// Decompress payload (public helper)
    pub fn decompress_payload(payload: &[u8], compression: WalCompression) -> WalResult<Vec<u8>> {
        compression_mod::decompress_payload(payload, compression)
    }

    // ── Getters and Setters ──
}

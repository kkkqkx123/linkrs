//! Transaction context behavior: mutation journal and derived log materialization

use std::sync::atomic::Ordering;

use super::TransactionContext;
use crate::error::TransactionError;
use crate::mutation_journal::{MutationResource, TransactionMutationRecord};
use crate::types::*;
#[cfg(test)]
use crate::wal::buffer::LocalWalBuffer;
use crate::wal::Timestamp;
use graphdb_core::types::CommitLsn;
impl TransactionContext {
    /// Number of redo entries staged by this transaction, derived from the
    /// canonical journal. This is a read-only view; see `materialize_redo`.
    pub fn redo_log_len(&self) -> usize {
        self.mutation_journal.read().total_redo_entries()
    }

    /// Materialize every redo entry held by the journal, in sequence order.
    ///
    /// Used by the commit path and savepoint export; the journal remains the
    /// single source of truth and no parallel redo log is maintained.
    pub fn materialize_redo(&self) -> Vec<crate::wal::TransactionWalEntry> {
        self.mutation_journal.read().redo_entries()
    }

    /// Materialize every outbox intent held by the journal, in sequence order.
    pub fn materialize_wal_intents(&self) -> Vec<graphdb_core::wal::OutboxIntent> {
        self.mutation_journal.read().wal_intents()
    }

    /// Rebuild the local WAL cache from the journal.
    ///
    /// Called after journal truncation (savepoint rollback, clear) and before
    /// the commit flush path so the buffer always mirrors the journal.
    pub fn rebuild_derived_logs(&self) {
        let journal = self.mutation_journal.read();
        let mut local = self.local_wal.lock();
        local.clear();
        for entry in journal.redo_entries() {
            let _ = local.append_full_entry(entry);
        }
        for intent in journal.wal_intents() {
            let _ = local.append_intent(intent);
        }
    }

    /// Direct access to the derived local WAL cache for tests.
    ///
    /// Production code must treat the journal as the single source of truth
    /// and use `rebuild_derived_logs` / `flush_local_wal` / `clear_derived_wal`
    /// instead of hand-editing the buffer.
    #[cfg(test)]
    pub fn local_wal_buffer(&self) -> parking_lot::MutexGuard<'_, LocalWalBuffer> {
        self.local_wal.lock()
    }

    /// Clear the derived WAL cache (mirrors journal truncation).
    pub fn clear_derived_wal(&self) {
        self.local_wal.lock().clear();
    }

    /// Number of buffered local WAL bytes (for metrics / backpressure).
    pub fn local_wal_bytes(&self) -> usize {
        self.local_wal.lock().buffered_bytes()
    }

    /// Whether the local WAL buffer is empty.
    pub fn is_local_wal_empty(&self) -> bool {
        self.local_wal.lock().is_empty()
    }

    /// Flush the materialized local WAL cache to a global writer.
    ///
    /// Callers must invoke `rebuild_derived_logs` first so the cache mirrors
    /// the journal; the flush itself only drains the cache.
    pub fn flush_local_wal(
        &self,
        writer: &mut crate::wal::LocalWalWriter,
    ) -> Result<CommitLsn, TransactionError> {
        let mut buf = self.local_wal.lock();
        buf.flush_to_writer(writer, self.id, self.durability)
            .map_err(|e| TransactionError::internal(e.to_string()))
    }

    /// Publish a complete mutation result in the canonical metadata order.
    ///
    /// The journal is the single source of truth: this method appends exactly
    /// one journal record and updates the write set, undo log and table
    /// markers. Redo entries and the local WAL buffer are not written here;
    /// they are materialized from the journal on demand via
    /// `materialize_redo` / `rebuild_derived_logs`.
    pub fn record_mutation(&self, mutation: MutationResult) -> Result<(), TransactionError> {
        self.can_execute()?;
        // Manager-level fail-fast for read-only transactions. Statement entry
        // points cannot know the statement kind up front (the query is parsed
        // after the statement scope opens), so the guard lives on the actual
        // write path instead of an intent flag: the query layer still rejects
        // write plans before execution, this rejects them if they ever reach
        // the journal, and commit certification rejects them at commit time.
        self.validate_write_allowed()?;
        let new_count = self.mutation_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.max_mutation_count > 0 && new_count > self.max_mutation_count {
            return Err(TransactionError::transaction_budget_exceeded(
                "mutation count",
                new_count,
                self.max_mutation_count,
            ));
        }

        if self.budget_warning_threshold > 0.0
            && self.max_mutation_count > 0
            && !self.mutation_warning_emitted.load()
            && new_count as f64 >= self.max_mutation_count as f64 * self.budget_warning_threshold
        {
            self.mutation_warning_emitted.store(true);
            log::warn!(
                "Transaction {} mutation count ({}) exceeds {:.0}% of limit ({})",
                self.id,
                new_count,
                self.budget_warning_threshold * 100.0,
                self.max_mutation_count,
            );
            self.budget_warnings
                .lock()
                .push(TransactionEvent::BudgetWarning {
                    txn_id: self.id,
                    resource: "mutation count".to_string(),
                    current: new_count,
                    limit: self.max_mutation_count,
                });
        }

        let undo_estimate = self.undo_bytes.fetch_add(64, Ordering::Relaxed) + 64;
        if self.max_undo_bytes > 0 && undo_estimate > self.max_undo_bytes {
            return Err(TransactionError::transaction_budget_exceeded(
                "undo bytes",
                undo_estimate,
                self.max_undo_bytes,
            ));
        }

        if self.budget_warning_threshold > 0.0
            && self.max_undo_bytes > 0
            && !self.undo_warning_emitted.load()
            && undo_estimate as f64 >= self.max_undo_bytes as f64 * self.budget_warning_threshold
        {
            self.undo_warning_emitted.store(true);
            log::warn!(
                "Transaction {} undo bytes ({}) exceeds {:.0}% of limit ({})",
                self.id,
                undo_estimate,
                self.budget_warning_threshold * 100.0,
                self.max_undo_bytes,
            );
            self.budget_warnings
                .lock()
                .push(TransactionEvent::BudgetWarning {
                    txn_id: self.id,
                    resource: "undo bytes".to_string(),
                    current: undo_estimate,
                    limit: self.max_undo_bytes,
                });
        }

        let resource = if mutation.resource != MutationResource::Unknown {
            mutation.resource
        } else if mutation.modified_table.is_some() {
            MutationResource::from_modified_table(mutation.modified_table.as_deref())
        } else if let Some(ref redo) = mutation.redo_entry {
            MutationResource::from_wal_op(redo.op_type)
        } else if !mutation.index_intents.is_empty() {
            MutationResource::SyncIntent
        } else {
            MutationResource::Unknown
        };

        let entity_keys = mutation.entity_keys.clone();
        let redo_with_seq = mutation.redo_entry.map(|mut e| {
            let seq = self.mutation_journal.read().next_sequence();
            e.transaction_id = Some(self.id);
            e.mutation_sequence = Some(seq);
            e
        });
        let journal_len_before = self.mutation_journal.read().len() as u64;
        let record = TransactionMutationRecord {
            sequence: journal_len_before,
            transaction_id: self.id,
            entity_keys: entity_keys.clone(),
            resource,
            undo: mutation.undo_entry.clone(),
            redo: redo_with_seq,
            index_intents: mutation.index_intents.clone(),
            modified_table: mutation.modified_table.clone(),
            write_timestamp: self.start_timestamp,
            commit_timestamp: None,
        };
        {
            let mut journal = self.mutation_journal.write();
            journal.push(record);
            if cfg!(debug_assertions) {
                if let Err(e) = journal.check_invariants() {
                    log::error!("journal invariant violated: {}", e);
                }
            }
        }

        for entity in entity_keys {
            match entity {
                MutationEntityKey::Vertex(vertex_id) => self.record_vertex_write(vertex_id),
                MutationEntityKey::Edge(edge) => self.record_edge_write(edge),
            }
        }
        if resource == MutationResource::Schema {
            if let Some(ref table) = mutation.modified_table {
                self.write_set.lock().record_schema_resource(table);
            } else {
                self.write_set.lock().record_schema_resource("schema");
            }
        }
        if resource == MutationResource::Index || resource == MutationResource::SyncIntent {
            for intent in &mutation.index_intents {
                self.record_index_write(&format!("{}", intent.mutation.ordering_key));
            }
        }
        if let Some(entry) = mutation.undo_entry {
            self.add_undo_log(entry)?;
        }
        if let Some(table) = mutation.modified_table {
            self.record_table_modification(&table);
        }
        self.write_validated.store(false);
        Ok(())
    }

    pub fn mutation_journal_len(&self) -> usize {
        self.mutation_journal.read().len()
    }

    pub fn next_mutation_sequence(&self) -> u64 {
        self.mutation_journal.read().next_sequence()
    }

    pub fn check_journal_invariants(&self) -> Result<(), String> {
        self.mutation_journal.read().check_invariants()
    }

    /// Publish the commit timestamp into the journal so every mutation that
    /// carried a write timestamp now also carries its commit timestamp.
    /// Must be called after the commit frontier has advanced and before
    /// storage finalization so pending vs committed history can be distinguished.
    pub fn publish_commit_timestamp(&self, commit_ts: Timestamp) {
        self.mutation_journal
            .write()
            .publish_commit_timestamp(commit_ts);
    }

    pub fn build_commit_descriptor(&self) -> crate::participant::TransactionCommitDescriptor {
        let ws = self.get_write_set();
        let rs = self.get_read_set();
        let journal = self.mutation_journal.read();
        let entry_count = journal.total_redo_entries();
        let intent_count = journal.total_intents();
        let first_sequence = journal.records().first().map(|r| r.sequence).unwrap_or(0);
        let range = 0..journal.len();
        drop(journal);
        let mut desc = crate::participant::TransactionCommitDescriptor::new(
            self.id,
            self.timestamp(),
            self.durability,
            ws,
        );
        desc.read_set = rs;
        desc.first_sequence = first_sequence;
        desc.entry_count = entry_count;
        desc.intent_count = intent_count;
        desc.journal_range = range;
        desc
    }
}

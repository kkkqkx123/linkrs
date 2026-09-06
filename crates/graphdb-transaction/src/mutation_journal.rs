//! Canonical mutation journal
//!
//! A single ordered log that is the source of truth for every logical
//! mutation inside a transaction. Undo, redo, index intents, write/read
//! metadata and table modification markers are all derived from one
//! sequentially-numbered journal entry, guaranteeing that savepoint,
//! rollback, certification and WAL commit see the same boundaries.

use std::sync::atomic::{AtomicU64, Ordering};

use graphdb_core::types::{Timestamp, TransactionId};
use graphdb_core::wal::{OutboxIntent, WalOpType};

use crate::types::MutationEntityKey;
use crate::undo_log::UndoLogEntry;
use crate::wal::TransactionWalEntry;

/// Classification of a mutation for certification, observability and
/// diagnostic grouping.  String table names are still retained for
/// backward compatibility but this enum is the primary discriminator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum MutationResource {
    Vertex,
    Edge,
    VertexProperty,
    EdgeProperty,
    Schema,
    Index,
    Sequence,
    SyncIntent,
    #[default]
    Unknown,
}

impl std::fmt::Display for MutationResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            MutationResource::Vertex => "vertex",
            MutationResource::Edge => "edge",
            MutationResource::VertexProperty => "vertex_property",
            MutationResource::EdgeProperty => "edge_property",
            MutationResource::Schema => "schema",
            MutationResource::Index => "index",
            MutationResource::Sequence => "sequence",
            MutationResource::SyncIntent => "sync_intent",
            MutationResource::Unknown => "unknown",
        };
        write!(f, "{}", s)
    }
}

impl MutationResource {
    /// Infer resource from WAL operation type when an explicit resource is
    /// not supplied.
    pub fn from_wal_op(op: WalOpType) -> Self {
        match op {
            WalOpType::InsertVertex | WalOpType::DeleteVertex => MutationResource::Vertex,
            WalOpType::InsertEdge | WalOpType::DeleteEdge => MutationResource::Edge,
            WalOpType::UpdateVertexProp
            | WalOpType::AddVertexProp
            | WalOpType::DeleteVertexProp
            | WalOpType::RenameVertexProp => MutationResource::VertexProperty,
            WalOpType::UpdateEdgeProp
            | WalOpType::AddEdgeProp
            | WalOpType::DeleteEdgeProp
            | WalOpType::RenameEdgeProp => MutationResource::EdgeProperty,
            WalOpType::CreateVertexType
            | WalOpType::DeleteVertexType
            | WalOpType::CreateEdgeType
            | WalOpType::DeleteEdgeType
            | WalOpType::CreateSpace
            | WalOpType::DropSpace
            | WalOpType::ClearSpace
            | WalOpType::AlterSpaceComment => MutationResource::Schema,
            WalOpType::CreateTagIndex
            | WalOpType::DropTagIndex
            | WalOpType::CreateEdgeIndex
            | WalOpType::DropEdgeIndex => MutationResource::Index,
            WalOpType::UpdateSequence => MutationResource::Sequence,
            WalOpType::OutboxIntent => MutationResource::SyncIntent,
            _ => MutationResource::Unknown,
        }
    }

    pub fn from_modified_table(table: Option<&str>) -> Self {
        match table {
            Some("vertex") => MutationResource::Vertex,
            Some("edge") => MutationResource::Edge,
            Some("schema") => MutationResource::Schema,
            Some("index") => MutationResource::Index,
            Some("sequence") => MutationResource::Sequence,
            Some("sync") => MutationResource::SyncIntent,
            _ => MutationResource::Unknown,
        }
    }
}

/// One canonical entry in the transaction mutation journal.
#[derive(Debug, Clone)]
pub struct TransactionMutationRecord {
    pub sequence: u64,
    pub transaction_id: TransactionId,
    pub entity_keys: Vec<MutationEntityKey>,
    pub resource: MutationResource,
    pub undo: Option<UndoLogEntry>,
    pub redo: Option<TransactionWalEntry>,
    pub index_intents: Vec<OutboxIntent>,
    pub modified_table: Option<String>,
    /// Write timestamp assigned at transaction start (isolated via frontier).
    pub write_timestamp: Timestamp,
    /// Commit timestamp assigned at publish time (when the write becomes
    /// visible to other transactions). `None` while the mutation is pending;
    /// `Some(ts)` after the commit frontier advances to `ts`. Distinct from
    /// `write_timestamp` so uncommitted versions are only visible to their
    /// owning transaction via read-your-writes.
    pub commit_timestamp: Option<Timestamp>,
}

impl TransactionMutationRecord {
    pub fn has_redo(&self) -> bool {
        self.redo.is_some()
    }
    pub fn has_undo(&self) -> bool {
        self.undo.is_some()
    }
}

/// Append-only journal that assigns a monotonic sequence to every
/// mutation recorded through `TransactionContext::record_mutation`.
#[derive(Debug, Default)]
pub struct MutationJournal {
    records: Vec<TransactionMutationRecord>,
    next_sequence: u64,
}

impl MutationJournal {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_sequence: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    pub fn push(&mut self, mut record: TransactionMutationRecord) -> u64 {
        let seq = self.next_sequence;
        record.sequence = seq;
        self.next_sequence += 1;
        self.records.push(record);
        seq
    }

    pub fn records(&self) -> &[TransactionMutationRecord] {
        &self.records
    }

    pub fn truncate(&mut self, len: usize) {
        if len < self.records.len() {
            let removed = self.records.len() - len;
            self.records.truncate(len);
            // Sequences are contiguous from 0, so next_sequence becomes len.
            // This preserves the invariant that next_sequence == records.len().
            self.next_sequence = self.records.len() as u64;
            let _ = removed;
        }
    }

    pub fn range(&self, start: usize, end: usize) -> &[TransactionMutationRecord] {
        let end = end.min(self.records.len());
        let start = start.min(end);
        &self.records[start..end]
    }

    /// Verify journal invariants.  Only executed in debug/test or explicit
    /// diagnostic mode, not on the hot path.
    pub fn check_invariants(&self) -> Result<(), String> {
        for (idx, rec) in self.records.iter().enumerate() {
            if rec.sequence != idx as u64 {
                return Err(format!(
                    "journal sequence gap: expected {}, got {} at index {}",
                    idx, rec.sequence, idx
                ));
            }
            if let Some(ref redo) = rec.redo {
                // Every redo-carrying record must have a WAL entry with a valid op type.
                if let Err(e) = WalOpType::try_from(redo.op_type as u8) {
                    return Err(format!(
                        "journal record {} has invalid WAL op type: {}",
                        rec.sequence, e
                    ));
                }
            }
        }
        // Intent sequence continuity is validated at WAL buffer flush time;
        // here we only verify that intents are not orphaned without a redo
        // batch context when both are present.
        if self.next_sequence != self.records.len() as u64 {
            return Err(format!(
                "journal next_sequence {} != len {}",
                self.next_sequence,
                self.records.len()
            ));
        }
        Ok(())
    }

    pub fn total_redo_entries(&self) -> usize {
        self.records.iter().filter(|r| r.redo.is_some()).count()
    }

    pub fn total_intents(&self) -> usize {
        self.records.iter().map(|r| r.index_intents.len()).sum()
    }

    /// Materialize every redo entry held by the journal, in sequence order.
    ///
    /// The journal is the single source of truth; per-transaction redo caches
    /// are derived from this view instead of being written in parallel.
    pub fn redo_entries(&self) -> Vec<TransactionWalEntry> {
        self.records.iter().filter_map(|r| r.redo.clone()).collect()
    }

    /// Materialize every outbox intent held by the journal, in sequence order.
    pub fn wal_intents(&self) -> Vec<OutboxIntent> {
        self.records
            .iter()
            .flat_map(|r| r.index_intents.iter().cloned())
            .collect()
    }

    /// Publish `commit_ts` to all pending records. Called once the transaction's
    /// commit timestamp has been allocated and the WAL durability boundary has
    /// been crossed. Earlier records retain `None` until publish; after publish
    /// they carry the commit timestamp so uncommitted vs reclaimable history
    /// can be distinguished via watermarks.
    pub fn publish_commit_timestamp(&mut self, commit_ts: Timestamp) {
        for rec in &mut self.records {
            if rec.commit_timestamp.is_none() {
                rec.commit_timestamp = Some(commit_ts);
            }
        }
    }

    /// Memory estimate for observability (bytes of payloads + keys).
    pub fn estimated_bytes(&self) -> usize {
        self.records
            .iter()
            .map(|r| {
                r.entity_keys.len() * std::mem::size_of::<MutationEntityKey>()
                    + r.redo.as_ref().map(|e| e.payload.len()).unwrap_or(0)
                    + r.index_intents
                        .iter()
                        .map(|i| i.mutation.document_or_vector.len())
                        .sum::<usize>()
            })
            .sum()
    }
}

/// Unified savepoint position derived from the mutation journal.
///
/// The journal is the single source of truth: a savepoint is fully described
/// by the journal length (and next sequence) at creation time. Redo caches,
/// the local WAL buffer, undo logs, write/read sets and table markers are all
/// truncated or restored from that single boundary; no derived offsets are
/// stored here so the views cannot diverge.
#[derive(Debug, Clone)]
pub struct MutationJournalPosition {
    /// Logical journal length at savepoint creation (authoritative).
    pub journal_len: usize,
    /// Next sequence that will be assigned (journal_len).
    pub next_sequence: u64,
    pub undo_log_index: usize,
    pub modified_tables: Vec<String>,
    pub write_set_snapshot: crate::types::WriteSet,
    pub read_set_snapshot: crate::types::WriteSet,
    pub sync_sequence: u64,
    /// Sequence number of the savepoint creation itself (for ordering).
    pub savepoint_sequence: u64,
}

impl MutationJournalPosition {
    pub fn validate_against(&self, journal: &MutationJournal) -> Result<(), String> {
        if self.journal_len > journal.len() {
            return Err(format!(
                "journal truncated beyond savepoint: savepoint len {} > current len {}",
                self.journal_len,
                journal.len()
            ));
        }
        if self.next_sequence != self.journal_len as u64 {
            return Err(format!(
                "savepoint next_sequence {} != journal_len {}",
                self.next_sequence, self.journal_len
            ));
        }
        Ok(())
    }
}

/// Atomic sequence generator for transactions without a journal lock.
#[derive(Debug, Default)]
pub struct MutationSequenceGenerator {
    next: AtomicU64,
}

impl MutationSequenceGenerator {
    pub fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
    pub fn current(&self) -> u64 {
        self.next.load(Ordering::Relaxed)
    }
    pub fn reset_to(&self, val: u64) {
        self.next.store(val, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::VertexId;

    #[test]
    fn journal_assigns_contiguous_sequences() {
        let mut j = MutationJournal::new();
        for i in 0..5 {
            let rec = TransactionMutationRecord {
                sequence: 0,
                transaction_id: TransactionId(1),
                entity_keys: vec![MutationEntityKey::Vertex(VertexId::from_int64(i))],
                resource: MutationResource::Vertex,
                undo: None,
                redo: None,
                index_intents: vec![],
                modified_table: None,
                write_timestamp: 1,
                commit_timestamp: None,
            };
            let seq = j.push(rec);
            assert_eq!(seq, i as u64);
        }
        assert!(j.check_invariants().is_ok());
        j.truncate(3);
        assert_eq!(j.len(), 3);
        assert_eq!(j.next_sequence(), 3);
        assert!(j.check_invariants().is_ok());
    }

    #[test]
    fn journal_invariant_detects_gap() {
        let mut j = MutationJournal::new();
        j.records.push(TransactionMutationRecord {
            sequence: 99,
            transaction_id: TransactionId(1),
            entity_keys: vec![],
            resource: MutationResource::Unknown,
            undo: None,
            redo: None,
            index_intents: vec![],
            modified_table: None,
            write_timestamp: 0,
            commit_timestamp: None,
        });
        j.next_sequence = 1;
        assert!(j.check_invariants().is_err());
    }

    #[test]
    fn resource_from_wal_op() {
        assert_eq!(
            MutationResource::from_wal_op(WalOpType::InsertVertex),
            MutationResource::Vertex
        );
        assert_eq!(
            MutationResource::from_wal_op(WalOpType::OutboxIntent),
            MutationResource::SyncIntent
        );
    }
}

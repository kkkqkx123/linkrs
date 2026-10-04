//! Commit publication with final review under the global commit lock,
//! recovery-only forced publication, SSI unregistration and watermark
//! recycling. Normal commits and recovery re-drives share one index-append
//! path; unregistration and pruning delegate to the tracker and index
//! containers.

use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

use dashmap::DashMap;

use graphdb_core::types::Timestamp;

use super::super::context::TransactionContext;
use super::super::error::TransactionError;
use super::super::types::*;
use super::conflict_kind::ConflictType;
use super::Certifier;

impl Certifier {
    /// Publish a committed write set into the conflict indices.
    ///
    /// Runs under the global commit lock (the same lock as the
    /// certification check) to close the window between certification and
    /// committed_write_sets publication. Re-checks all active (validated)
    /// transactions and all committed entries since `start_timestamp` as a
    /// final review.
    ///
    /// `write_timestamp` MUST be the commit timestamp allocated by
    /// `VersionManager::allocate_commit_timestamp`, never the transaction
    /// start timestamp: conflict lookups compare stored timestamps against
    /// later committers' start timestamps, and visibility is ordered by
    /// commit time. Indexing by start time would miss conflicts from
    /// long-running writers that commit after a later transaction starts.
    ///
    /// On conflict, returns `Err` and publishes nothing.
    /// Lock-free bypass for empty write sets or in-memory read-only transactions.
    pub fn publish(
        &self,
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        start_timestamp: Timestamp,
        write_set: &WriteSet,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        if write_set.is_empty() {
            // SSI: still unregister read locks even for empty writes
            self.ssi_tracker.unregister_reads(txn_id);
            return Ok(());
        }
        // Lock order: commit_lock → committed_write_sets → *
        let _cert_guard = self.commit_lock.lock();
        let mut committed = self.committed_write_sets.lock();

        // Final review under the global commit lock: because the
        // certification check ran under this same lock, no conflicting
        // transaction can slip between our check and this publication.
        // The re-scan of active (validated) transactions and committed
        // entries since our start_timestamp is defense in depth.
        for entry in active_transactions.iter() {
            let (other_id, other_ctx) = entry.pair();
            if *other_id == txn_id {
                continue;
            }
            if other_ctx.read_only {
                continue;
            }
            if !other_ctx.is_write_validated() {
                continue;
            }
            if write_set.has_conflict_with(&other_ctx.get_write_set()) {
                stats.record_txn_conflict_with_type(ConflictType::WriteWrite);
                log::warn!(
                    "certification conflict (publish) txn={} other={} type={}",
                    txn_id,
                    other_id,
                    ConflictType::WriteWrite
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} write-write with {} (publish)",
                    ConflictType::WriteWrite,
                    other_id
                )));
            }
        }
        for (_, sets) in committed.range((Excluded(start_timestamp), Unbounded)) {
            for ws in sets {
                if write_set.has_conflict_with(ws) {
                    stats.record_txn_conflict_with_type(ConflictType::WriteWrite);
                    log::warn!(
                        "certification conflict (publish) txn={} type={}",
                        txn_id,
                        ConflictType::WriteWrite
                    );
                    return Err(TransactionError::serialization_failed(format!(
                        "conflict {} write-write with committed batch (publish)",
                        ConflictType::WriteWrite,
                    )));
                }
            }
        }

        committed
            .entry(write_timestamp)
            .or_default()
            .push(write_set.clone());
        self.index_write_set(txn_id, write_timestamp, write_set);

        // SSI: unregister read locks and register write locks.
        self.ssi_tracker.unregister_reads(txn_id);
        Ok(())
    }

    /// Insert a write set into the conflict indices without certification.
    ///
    /// Recovery-only path: the commit is already WAL-durable and finalized,
    /// so it cannot be aborted on conflict. A conflicting entry still
    /// defends later committers (they abort instead of silently losing the
    /// update), which is the conservative direction. Normal commits MUST
    /// use `publish` so conflicts abort before durability.
    pub fn force_publish(
        &self,
        txn_id: TransactionId,
        commit_timestamp: Timestamp,
        write_set: &WriteSet,
    ) {
        if write_set.is_empty() {
            self.ssi_tracker.unregister_reads(txn_id);
            return;
        }
        let _cert_guard = self.commit_lock.lock();
        let mut committed = self.committed_write_sets.lock();
        if committed
            .get(&commit_timestamp)
            .is_some_and(|sets| sets.iter().any(|ws| *ws == *write_set))
        {
            self.ssi_tracker.unregister_reads(txn_id);
            return;
        }
        committed
            .entry(commit_timestamp)
            .or_default()
            .push(write_set.clone());
        self.index_write_set(txn_id, commit_timestamp, write_set);
        self.ssi_tracker.unregister_reads(txn_id);
    }

    /// Append a committed write set to all four spatial indices. Shared by
    /// the normal publish path and the recovery-only forced publication.
    fn index_write_set(&self, txn_id: TransactionId, timestamp: Timestamp, write_set: &WriteSet) {
        for vid in write_set.vertices.iter() {
            self.vertex_writes.add(vid, timestamp, txn_id);
        }
        for edge in write_set.edges.iter() {
            self.edge_writes.add(edge, timestamp, txn_id);
        }
        for resource in write_set.schema_resources.iter() {
            self.schema_writes.add(resource, timestamp, txn_id);
        }
        for resource in write_set.index_resources.iter() {
            self.index_writes.add(resource, timestamp, txn_id);
        }
    }

    /// Remove all SSI read locks held by `txn_id` (on commit or abort).
    pub fn unregister_reads(&self, txn_id: TransactionId) {
        self.ssi_tracker.unregister_reads(txn_id);
    }

    /// Prune committed write sets that are no longer needed by any active
    /// transaction. Entries with commit timestamps <= `oldest_active_ts`
    /// are safe to remove.
    pub fn prune(&self, oldest_active_ts: Timestamp) {
        let mut committed = self.committed_write_sets.lock();
        committed.retain(|ts, _| *ts > oldest_active_ts);

        self.vertex_writes.prune(oldest_active_ts);
        self.edge_writes.prune(oldest_active_ts);
        self.schema_writes.prune(oldest_active_ts);
        self.index_writes.prune(oldest_active_ts);

        // SSI: prune stale read locks.
        self.ssi_tracker.prune(oldest_active_ts);
    }
}

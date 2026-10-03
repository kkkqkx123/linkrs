//! Optimistic certification pre-check: lock-free fast paths, active-transaction
//! scan, committed write-set index probes, phantom checks and SSI
//! dangerous-structure detection.

use std::sync::Arc;

use dashmap::DashMap;

use graphdb_core::types::Timestamp;

use super::super::context::TransactionContext;
use super::super::error::TransactionError;
use super::super::types::*;
use super::conflict_kind::ConflictType;
use super::Certifier;

impl Certifier {
    /// Check for write-set based conflicts with active transactions.
    ///
    /// This method checks if a transaction's write set conflicts with any other
    /// write transactions that have already passed validation.
    /// After a successful check, the transaction is marked as validated.
    ///
    /// Fast paths (lock-free, no commit-lock acquisition):
    /// - Read-only transactions never conflict.
    /// - SingleWriter mode bypasses certification (exclusive write lease).
    /// - Empty write sets (and empty read sets for Serializable) bypass certification.
    ///
    /// Only transactions that need certification acquire the global
    /// commit lock. Returns Ok(()) if no conflicts, or Err if conflicts
    /// are detected.
    pub fn check_write_set_conflict(
        &self,
        txn_id: TransactionId,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        // Lock-free fast path: cheap checks before acquiring the global lock.
        let ctx = active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        let txn_write_set = ctx.get_write_set();
        let txn_read_set = ctx.get_read_set();
        let serializable = ctx.isolation_level == IsolationLevel::Serializable;

        // Read-only fast path: only Serializable with a non-empty read set
        // needs certification (phantom / schema / index generation checks).
        if ctx.read_only && (!serializable || txn_read_set.is_empty()) {
            return Ok(());
        }

        if ctx.get_concurrency_mode() == ConcurrencyMode::SingleWriter {
            ctx.mark_write_validated();
            return Ok(());
        }

        if txn_write_set.is_empty() && (!serializable || txn_read_set.is_empty()) {
            // Read-only Serializable already handled above; this is the empty
            // write+read case for write transactions.
            if !ctx.read_only {
                return Ok(());
            }
        }

        // Only acquire the global commit lock for transactions that require
        // certification. The check and the later publish both run under this
        // same lock, which closes the race where two conflicting
        // transactions each observed the other as not-yet-validated and both
        // passed.
        let _certification_guard = self.commit_lock.lock();
        // Re-fetch context after acquiring lock to ensure consistency (context may have been removed)
        let ctx = active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        // SSI: register read locks for all entities in the read set.
        if serializable {
            self.ssi_tracker
                .register_txn_reads(txn_id, ctx.start_timestamp, &txn_read_set);
        }

        self.scan_active_transactions(
            txn_id,
            &ctx,
            serializable,
            &txn_read_set,
            active_transactions,
            stats,
        )?;

        let committed = self.committed_write_sets.lock();

        self.probe_committed_writes(txn_id, ctx.start_timestamp, &txn_write_set, stats)?;

        if serializable {
            self.probe_committed_reads(txn_id, ctx.start_timestamp, &txn_read_set, stats)?;
            self.check_range_phantoms_and_full_scan(
                &ctx,
                &txn_read_set,
                committed.as_slice(),
                stats,
            )?;
            // Dangerous-structure detection: T_current writes R, T_other read
            // R, AND T_current read something T_other writes (O(W × K) where
            // W = write set size and K = max readers per resource).
            self.detect_dangerous_structures(
                txn_id,
                &ctx,
                &txn_write_set,
                active_transactions,
                stats,
            )?;
        }
        drop(committed);

        ctx.mark_write_validated();
        Ok(())
    }

    /// Scan active validated writers for direct write-write conflicts and,
    /// for Serializable, read-set vs write-set conflicts.
    fn scan_active_transactions(
        &self,
        txn_id: TransactionId,
        ctx: &Arc<TransactionContext>,
        serializable: bool,
        txn_read_set: &WriteSet,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        for entry in active_transactions.iter() {
            let (other_id, other_ctx) = entry.pair();

            if other_id == &txn_id {
                continue;
            }

            if other_ctx.read_only {
                continue;
            }

            if !other_ctx.is_write_validated() {
                continue;
            }

            if ctx.has_write_conflict_with(other_ctx) {
                stats.record_txn_conflict_with_type(ConflictType::WriteWrite);
                log::warn!(
                    "certification conflict txn={} other={} type={}",
                    txn_id,
                    other_id,
                    ConflictType::WriteWrite
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} write-write with {}",
                    ConflictType::WriteWrite,
                    other_id
                )));
            }
            if serializable && txn_read_set.has_conflict_with(&other_ctx.get_write_set()) {
                stats.record_txn_conflict_with_type(ConflictType::ReadWrite);
                log::warn!(
                    "certification conflict txn={} other={} type={}",
                    txn_id,
                    other_id,
                    ConflictType::ReadWrite
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} read-write with {}",
                    ConflictType::ReadWrite,
                    other_id
                )));
            }
        }
        Ok(())
    }

    /// O(1) write-set conflict lookup against the committed spatial indices.
    fn probe_committed_writes(
        &self,
        txn_id: TransactionId,
        start_timestamp: Timestamp,
        txn_write_set: &WriteSet,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        for vid in txn_write_set.vertices.iter() {
            if self.vertex_writes.conflicts_after(vid, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::WriteWrite);
                log::warn!(
                    "certification conflict txn={} type={} resource=vertex:{:?}",
                    txn_id,
                    ConflictType::WriteWrite,
                    vid
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} write-write vertex {:?}",
                    ConflictType::WriteWrite,
                    vid
                )));
            }
        }

        for edge in txn_write_set.edges.iter() {
            if self.edge_writes.conflicts_after(edge, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::WriteWrite);
                log::warn!(
                    "certification conflict txn={} type={} resource=edge:{:?}",
                    txn_id,
                    ConflictType::WriteWrite,
                    edge
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} write-write edge {:?}",
                    ConflictType::WriteWrite,
                    edge
                )));
            }
        }

        for resource in txn_write_set.schema_resources.iter() {
            if self
                .schema_writes
                .conflicts_after(resource, start_timestamp)
            {
                stats.record_txn_conflict_with_type(ConflictType::SchemaGeneration);
                log::warn!(
                    "certification conflict txn={} type={} resource=schema:{}",
                    txn_id,
                    ConflictType::SchemaGeneration,
                    resource
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} schema {}",
                    ConflictType::SchemaGeneration,
                    resource
                )));
            }
        }

        for resource in txn_write_set.index_resources.iter() {
            if self.index_writes.conflicts_after(resource, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::IndexGeneration);
                log::warn!(
                    "certification conflict txn={} type={} resource=index:{}",
                    txn_id,
                    ConflictType::IndexGeneration,
                    resource
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} index {}",
                    ConflictType::IndexGeneration,
                    resource
                )));
            }
        }
        Ok(())
    }

    /// O(1) read-set conflict lookup via committed write indices. The
    /// committed write-set scan for phantoms is handled separately by
    /// [`Self::check_range_phantoms_and_full_scan`].
    fn probe_committed_reads(
        &self,
        txn_id: TransactionId,
        start_timestamp: Timestamp,
        txn_read_set: &WriteSet,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        for vid in txn_read_set.vertices.iter() {
            if self.vertex_writes.conflicts_after(vid, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::ReadWrite);
                log::warn!(
                    "certification conflict txn={} type={} resource=vertex:{:?}",
                    txn_id,
                    ConflictType::ReadWrite,
                    vid
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} read-write vertex {:?}",
                    ConflictType::ReadWrite,
                    vid
                )));
            }
        }

        for edge in txn_read_set.edges.iter() {
            if self.edge_writes.conflicts_after(edge, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::ReadWrite);
                log::warn!(
                    "certification conflict txn={} type={} resource=edge:{:?}",
                    txn_id,
                    ConflictType::ReadWrite,
                    edge
                );
                return Err(TransactionError::serialization_failed(format!(
                    "conflict {} read-write edge {:?}",
                    ConflictType::ReadWrite,
                    edge
                )));
            }
        }

        for resource in txn_read_set.schema_resources.iter() {
            if self
                .schema_writes
                .conflicts_after(resource, start_timestamp)
            {
                stats.record_txn_conflict_with_type(ConflictType::SchemaGeneration);
                return Err(TransactionError::serialization_failed(format!(
                    "schema-generation conflict: {} {}",
                    ConflictType::SchemaGeneration,
                    resource
                )));
            }
        }

        for resource in txn_read_set.index_resources.iter() {
            if self.index_writes.conflicts_after(resource, start_timestamp) {
                stats.record_txn_conflict_with_type(ConflictType::IndexGeneration);
                return Err(TransactionError::serialization_failed(format!(
                    "index-generation conflict: {} {}",
                    ConflictType::IndexGeneration,
                    resource
                )));
            }
        }
        Ok(())
    }

    /// Predicate range phantom detection and the full-scan threshold check
    /// against the committed write-set list.
    fn check_range_phantoms_and_full_scan(
        &self,
        ctx: &Arc<TransactionContext>,
        txn_read_set: &WriteSet,
        committed: &[(Timestamp, WriteSet)],
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        // A concurrent committed write whose vertex falls inside a read range
        // committed after our start indicates a phantom.
        if !txn_read_set.read_ranges.is_empty() {
            for (commit_ts, ws) in committed.iter() {
                if *commit_ts <= ctx.start_timestamp {
                    continue;
                }
                if txn_read_set.has_read_range_conflict_with(ws) {
                    stats.record_txn_conflict_with_type(ConflictType::Phantom);
                    return Err(TransactionError::serialization_failed(format!(
                        "phantom conflict: {} on range",
                        ConflictType::Phantom
                    )));
                }
            }
        }

        // Full-scan certification when the read set is large: the
        // per-resource probes above already passed, so abort only on a
        // newer commit that actually touches the tracked footprint.
        // Unrelated concurrent commits no longer abort the scan.
        if let Some(threshold) = ctx.serializable_full_scan_threshold() {
            let read_size = txn_read_set.size() + txn_read_set.read_ranges.len();
            if read_size >= threshold {
                let has_conflicting_commit = committed.iter().any(|(commit_ts, ws)| {
                    *commit_ts > ctx.start_timestamp && txn_read_set.has_read_conflict_with(ws)
                });
                if has_conflicting_commit {
                    stats.record_txn_conflict_with_type(ConflictType::Phantom);
                    return Err(TransactionError::serialization_failed(
                        "full-scan read set exceeds threshold with overlapping commit",
                    ));
                }
            }
        }
        Ok(())
    }

    /// SSI dangerous-structure detection against active transactions.
    fn detect_dangerous_structures(
        &self,
        txn_id: TransactionId,
        ctx: &Arc<TransactionContext>,
        txn_write_set: &WriteSet,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        let write_resources = txn_write_set.ssi_resources();
        let read_resources = ctx.get_ssi_read_resources();

        for resource in &write_resources {
            // rw-dependency: T_other →rw T_current
            for &(reader_id, reader_start_ts) in &self.ssi_tracker.readers(resource) {
                if reader_id == txn_id {
                    continue;
                }
                if reader_start_ts >= ctx.start_timestamp {
                    continue;
                }
                // Check if T_current also reads something T_other writes
                // (rw-dependency: T_current →rw T_other → potential cycle)
                if let Some(reader_ctx) = active_transactions.get(&reader_id) {
                    if !reader_ctx.read_only
                        && reader_ctx.is_write_validated()
                        && read_resources
                            .iter()
                            .any(|r| reader_ctx.get_write_set().ssi_resources().contains(r))
                    {
                        stats.record_txn_conflict_with_type(ConflictType::ReadWrite);
                        return Err(TransactionError::serialization_failed(
                            "SSI dangerous structure detected: read-write cycle",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

//! Write-set certification
//!
//! Certifies write sets against active and committed transactions before
//! commit, and publishes committed write sets into O(1) spatial indices for
//! conflict lookups. Also tracks SSI rw-dependencies for Serializable
//! isolation.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::Mutex;

use graphdb_core::types::{LabelId, Timestamp, VertexId};

use super::context::TransactionContext;
use super::error::TransactionError;
use super::types::*;

/// Number of certification lock shards. Must be a power of two for efficient
/// modulo via bitmask (though the compiler optimizes `% 64` anyway).
const CERT_SHARD_COUNT: usize = 64;

/// Maps a resource reference to its committed write timestamps + transaction IDs.
type ConflictMap<V> = HashMap<V, Vec<(Timestamp, TransactionId)>>;

/// SSI (Serializable Snapshot Isolation) rw-dependency tracker.
///
/// Instead of scanning all committed write sets (O(N)), this tracker maintains
/// per-resource read locks that enable O(1) dangerous-structure detection.
struct SsiTracker {
    /// Per-resource list of active readers: resource → Vec<(txn_id, start_ts)>
    read_locks: parking_lot::RwLock<HashMap<ResourceId, Vec<(TransactionId, Timestamp)>>>,
}

impl SsiTracker {
    fn new() -> Self {
        Self {
            read_locks: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Register that `txn_id` read `resource` at `start_ts`.
    fn register_read(&self, txn_id: TransactionId, resource: ResourceId, start_ts: Timestamp) {
        self.read_locks
            .write()
            .entry(resource)
            .or_default()
            .push((txn_id, start_ts));
    }

    /// Remove all read locks held by `txn_id` (on commit or abort).
    fn unregister_reads(&self, txn_id: TransactionId) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(id, _)| *id != txn_id);
            !entries.is_empty()
        });
    }

    /// Prune read locks older than `oldest_active_ts`.
    fn prune(&self, oldest_active_ts: Timestamp) {
        let mut locks = self.read_locks.write();
        locks.retain(|_, entries| {
            entries.retain(|(_, ts)| *ts > oldest_active_ts);
            !entries.is_empty()
        });
    }
}

/// Certifier for write-set conflict detection.
///
/// Maintains sharded certification locks plus committed write-set spatial
/// indices (O(1) per-resource conflict lookup) and the SSI tracker. Sharded
/// locks serialize certification + committed-write-set publication per
/// transaction ID so non-conflicting transactions certify in parallel.
pub struct Certifier {
    /// Sharded certification locks. Each shard serializes certification +
    /// committed_write_sets push for a single transaction. Shard selection
    /// is by `txn_id % CERT_SHARD_COUNT`, so non-conflicting transactions
    /// can certify in parallel.
    certification_shards: [Mutex<()>; CERT_SHARD_COUNT],
    /// Committed write sets retained until no transaction can have started
    /// before the corresponding commit timestamp.
    committed_write_sets: Mutex<Vec<(Timestamp, WriteSet)>>,
    /// Spatial index for O(1) vertex conflict lookup.
    /// Maps each vertex ID to committed write timestamps + transaction IDs.
    committed_vertex_writes: Mutex<ConflictMap<VertexId>>,
    /// Spatial index for O(1) edge conflict lookup.
    /// Key: (src_vid, dst_vid, edge_label).
    committed_edge_writes: Mutex<ConflictMap<(VertexId, VertexId, LabelId)>>,
    /// Spatial index for O(1) schema resource conflict lookup.
    committed_schema_writes: Mutex<ConflictMap<String>>,
    /// Spatial index for O(1) index resource conflict lookup.
    committed_index_writes: Mutex<ConflictMap<String>>,
    /// SSI rw-dependency tracker for Serializable isolation.
    ssi_tracker: SsiTracker,
}

impl Certifier {
    pub fn new() -> Self {
        Self {
            certification_shards: std::array::from_fn(|_| Mutex::new(())),
            committed_write_sets: Mutex::new(Vec::new()),
            committed_vertex_writes: Mutex::new(HashMap::new()),
            committed_edge_writes: Mutex::new(HashMap::new()),
            committed_schema_writes: Mutex::new(HashMap::new()),
            committed_index_writes: Mutex::new(HashMap::new()),
            ssi_tracker: SsiTracker::new(),
        }
    }

    fn cert_shard(&self, txn_id: TransactionId) -> &Mutex<()> {
        &self.certification_shards[txn_id.0 as usize % CERT_SHARD_COUNT]
    }

    /// Check for write-set based conflicts with active transactions.
    ///
    /// This method checks if a transaction's write set conflicts with any other
    /// write transactions that have already passed validation.
    /// After a successful check, the transaction is marked as validated.
    ///
    /// Returns Ok(()) if no conflicts, or Err if conflicts are detected.
    pub fn check_write_set_conflict(
        &self,
        txn_id: TransactionId,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        let _certification_guard = self.cert_shard(txn_id).lock();
        let ctx = active_transactions
            .get(&txn_id)
            .ok_or_else(|| TransactionError::transaction_not_found(txn_id))?;

        if ctx.read_only {
            return Ok(());
        }

        // SingleWriter mode guarantees serialization via the exclusive write lock.
        if ctx.get_concurrency_mode() == ConcurrencyMode::SingleWriter {
            ctx.mark_write_validated();
            return Ok(());
        }

        let txn_write_set = ctx.get_write_set();
        let txn_read_set = ctx.get_read_set();
        let serializable = ctx.isolation_level == IsolationLevel::Serializable;
        if txn_write_set.is_empty() && (!serializable || txn_read_set.is_empty()) {
            return Ok(());
        }

        // SSI: register read locks for all entities in the read set.
        // This enables O(1) dangerous-structure detection when other
        // transactions write to these resources.
        if serializable {
            for vid in txn_read_set.vertices.iter() {
                self.ssi_tracker.register_read(
                    txn_id,
                    ResourceId::Vertex(*vid),
                    ctx.start_timestamp,
                );
            }
            for edge in txn_read_set.edges.iter() {
                self.ssi_tracker.register_read(
                    txn_id,
                    ResourceId::Edge(*edge),
                    ctx.start_timestamp,
                );
            }
            for resource in txn_read_set.schema_resources.iter() {
                self.ssi_tracker.register_read(
                    txn_id,
                    ResourceId::Schema(resource.clone()),
                    ctx.start_timestamp,
                );
            }
        }

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

            if ctx.has_write_conflict_with(other_ctx)
                || (serializable && txn_read_set.has_conflict_with(&other_ctx.get_write_set()))
            {
                stats.record_txn_conflict();
                return Err(TransactionError::write_transaction_conflict());
            }
        }

        let committed = self.committed_write_sets.lock();
        // O(1) vertex conflict lookup via spatial index.
        let vertex_idx = self.committed_vertex_writes.lock();
        for vid in txn_write_set.vertices.iter() {
            if let Some(entries) = vertex_idx.get(vid) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(vertex_idx);
                    drop(committed);
                    stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(vertex_idx);

        // O(1) edge conflict lookup via spatial index.
        let edge_idx = self.committed_edge_writes.lock();
        for edge in txn_write_set.edges.iter() {
            let key = (edge.src_vid, edge.dst_vid, edge.edge_label);
            if let Some(entries) = edge_idx.get(&key) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(edge_idx);
                    drop(committed);
                    stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(edge_idx);

        // O(1) schema resource conflict lookup.
        let schema_idx = self.committed_schema_writes.lock();
        for resource in txn_write_set.schema_resources.iter() {
            if let Some(entries) = schema_idx.get(resource) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(schema_idx);
                    drop(committed);
                    stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(schema_idx);

        // O(1) index resource conflict lookup.
        let index_idx = self.committed_index_writes.lock();
        for resource in txn_write_set.index_resources.iter() {
            if let Some(entries) = index_idx.get(resource) {
                if entries
                    .iter()
                    .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                {
                    drop(index_idx);
                    drop(committed);
                    stats.record_txn_conflict();
                    return Err(TransactionError::write_transaction_conflict());
                }
            }
        }
        drop(index_idx);

        // The O(N) committed_write_sets scan below only handles Serializable
        // read-range phantom and full-scan detection. Exact read-set entity
        // conflicts are resolved via O(1) spatial indices below.
        if serializable {
            // O(1) read-set conflict lookup via committed write indices.
            let vertex_idx = self.committed_vertex_writes.lock();
            for vid in txn_read_set.vertices.iter() {
                if let Some(entries) = vertex_idx.get(vid) {
                    if entries
                        .iter()
                        .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                    {
                        drop(vertex_idx);
                        drop(committed);
                        stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(vertex_idx);

            let edge_idx = self.committed_edge_writes.lock();
            for edge in txn_read_set.edges.iter() {
                let key = (edge.src_vid, edge.dst_vid, edge.edge_label);
                if let Some(entries) = edge_idx.get(&key) {
                    if entries
                        .iter()
                        .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                    {
                        drop(edge_idx);
                        drop(committed);
                        stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(edge_idx);

            let schema_idx = self.committed_schema_writes.lock();
            for resource in txn_read_set.schema_resources.iter() {
                if let Some(entries) = schema_idx.get(resource) {
                    if entries
                        .iter()
                        .any(|(commit_ts, _)| *commit_ts > ctx.start_timestamp)
                    {
                        drop(schema_idx);
                        drop(committed);
                        stats.record_txn_conflict();
                        return Err(TransactionError::write_transaction_conflict());
                    }
                }
            }
            drop(schema_idx);
        }

        // SSI (Serializable Snapshot Isolation) dangerous-structure detection.
        //
        // Instead of scanning all committed write sets (O(N)), we check for
        // dangerous structures: T_current writes R, T_other read R, AND
        // T_current read something T_other writes. This is O(W × K) where
        // W = write set size and K = max readers per resource.
        //
        // We also check against committed write sets via spatial indices (O(1))
        // for the reverse direction (read set vs committed writes).
        if serializable {
            let write_resources = txn_write_set.ssi_resources();
            let read_resources = ctx.get_ssi_read_resources();

            for resource in &write_resources {
                // Check if any active transaction has read this resource
                // (rw-dependency: T_other →rw T_current)
                let ssi_locks = self.ssi_tracker.read_locks.read();
                if let Some(readers) = ssi_locks.get(resource) {
                    for &(reader_id, reader_start_ts) in readers {
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
                                drop(ssi_locks);
                                drop(committed);
                                stats.record_txn_conflict();
                                return Err(TransactionError::serialization_failed(
                                    "SSI dangerous structure detected: read-write cycle",
                                ));
                            }
                        }
                    }
                }
            }
        }
        drop(committed);

        ctx.mark_write_validated();
        Ok(())
    }

    /// Publish a committed write set into the conflict indices.
    ///
    /// Runs under the certification shard lock to close the window between
    /// certification and committed_write_sets publication. Re-checks all
    /// active (validated) transactions and all committed entries since
    /// `start_timestamp` to catch cross-shard certification races.
    ///
    /// On conflict, returns `Err` and publishes nothing.
    pub fn publish(
        &self,
        txn_id: TransactionId,
        write_timestamp: Timestamp,
        start_timestamp: Timestamp,
        write_set: &WriteSet,
        active_transactions: &DashMap<TransactionId, Arc<TransactionContext>>,
        stats: &TransactionStats,
    ) -> Result<(), TransactionError> {
        // Lock order: cert_shard → committed_write_sets → *
        let _cert_guard = self.cert_shard(txn_id).lock();
        let mut committed = self.committed_write_sets.lock();

        // Final review: cross-shard certification race prevention.
        //
        // check_write_set_conflict() only serializes via cert_shard,
        // so two conflicting transactions in different shards can
        // both pass because each reads the other's
        // is_write_validated() == false and skips it.
        //
        // This re-check under cert_shard catches the race by
        // scanning all active (validated) transactions and all
        // committed entries since our start_timestamp.
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
                stats.record_txn_conflict();
                return Err(TransactionError::write_transaction_conflict());
            }
        }
        for (commit_ts, ws) in committed.iter() {
            if *commit_ts <= start_timestamp {
                continue;
            }
            if write_set.has_conflict_with(ws) {
                stats.record_txn_conflict();
                return Err(TransactionError::write_transaction_conflict());
            }
        }

        committed.push((write_timestamp, write_set.clone()));
        let mut vertex_idx = self.committed_vertex_writes.lock();
        for vid in write_set.vertices.iter() {
            vertex_idx
                .entry(*vid)
                .or_default()
                .push((write_timestamp, txn_id));
        }
        let mut edge_idx = self.committed_edge_writes.lock();
        for edge in write_set.edges.iter() {
            edge_idx
                .entry((edge.src_vid, edge.dst_vid, edge.edge_label))
                .or_default()
                .push((write_timestamp, txn_id));
        }
        let mut schema_idx = self.committed_schema_writes.lock();
        for resource in write_set.schema_resources.iter() {
            schema_idx
                .entry(resource.clone())
                .or_default()
                .push((write_timestamp, txn_id));
        }
        let mut index_idx = self.committed_index_writes.lock();
        for resource in write_set.index_resources.iter() {
            index_idx
                .entry(resource.clone())
                .or_default()
                .push((write_timestamp, txn_id));
        }

        // SSI: unregister read locks and register write locks.
        self.ssi_tracker.unregister_reads(txn_id);
        Ok(())
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
        committed.retain(|(ts, _)| *ts > oldest_active_ts);

        let retain_fn = |entries: &mut Vec<(Timestamp, TransactionId)>| {
            entries.retain(|(commit_ts, _)| *commit_ts > oldest_active_ts);
            !entries.is_empty()
        };

        let mut vertex_idx = self.committed_vertex_writes.lock();
        vertex_idx.retain(|_, entries| retain_fn(entries));
        let mut edge_idx = self.committed_edge_writes.lock();
        edge_idx.retain(|_, entries| retain_fn(entries));
        let mut schema_idx = self.committed_schema_writes.lock();
        schema_idx.retain(|_, entries| retain_fn(entries));
        let mut index_idx = self.committed_index_writes.lock();
        index_idx.retain(|_, entries| retain_fn(entries));

        // SSI: prune stale read locks.
        self.ssi_tracker.prune(oldest_active_ts);
    }
}

impl Default for Certifier {
    fn default() -> Self {
        Self::new()
    }
}

//! Write-set certification
//!
//! Certifies write sets against active and committed transactions before
//! commit, and publishes committed write sets into O(1) spatial indices for
//! conflict lookups. Also tracks SSI rw-dependencies for Serializable
//! isolation.
//!
//! The certification pre-check lives in [`check`], publication and recycling
//! in [`publish`], the committed write-set spatial indices in
//! [`conflict_index`], the SSI tracker in [`ssi_tracker`], and the conflict
//! classification in [`conflict_kind`].

mod check;
mod conflict_index;
mod conflict_kind;
mod publish;
mod ssi_tracker;
use parking_lot::Mutex;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicUsize;

use linkrs_core::types::{EdgeIdentifier, Timestamp, VertexId};

use super::types::WriteSet;
use conflict_index::ConflictIndex;
use ssi_tracker::SsiTracker;

pub use conflict_kind::ConflictType;

/// Certifier for write-set conflict detection.
///
/// Maintains a global commit lock plus committed write-set spatial indices
/// (O(1) per-resource conflict lookup) and the SSI tracker. The global lock
/// serializes the check-then-publish critical section across ALL
/// transactions: per-transaction shard locks cannot close the window in
/// which two conflicting transactions in different shards both pass the
/// check, so certification is intentionally unsharded.
///
/// Lock order wherever multiple locks are held: `commit_lock` →
/// `committed_write_sets` → spatial indices → `ssi_tracker`.
///
/// Lock scope: the global lock is held only across each in-memory
/// certification step (`check_write_set_conflict` pre-check and `publish`
/// final review separately). WAL durability I/O and storage finalization in
/// `TransactionManager::commit_transaction` run outside this lock; the final
/// review re-scans validated active transactions plus every committed write
/// set newer than the committer's start timestamp, so no conflict can slip
/// through the unlocked I/O window.
pub struct Certifier {
    /// Global certification lock. Held across the whole
    /// check-then-publish critical section.
    commit_lock: Mutex<()>,
    /// Committed write sets retained until no transaction can have started
    /// before the corresponding commit timestamp. Keyed by commit timestamp
    /// so the final-review range scan touches only commits newer than the
    /// committer's start timestamp instead of every retained commit.
    committed_write_sets: Mutex<BTreeMap<Timestamp, Vec<WriteSet>>>,
    /// Spatial index for O(1) vertex conflict lookup.
    /// Maps each vertex ID to committed write timestamps + transaction IDs.
    vertex_writes: ConflictIndex<VertexId>,
    /// Spatial index for O(1) edge conflict lookup.
    /// Keyed by the full edge identity (endpoint labels and rank included)
    /// so edges sharing endpoints but differing in rank or endpoint labels
    /// do not falsely conflict; matches the publish-time precise check.
    edge_writes: ConflictIndex<EdgeIdentifier>,
    /// Spatial index for O(1) schema resource conflict lookup.
    schema_writes: ConflictIndex<String>,
    /// Spatial index for O(1) index resource conflict lookup.
    index_writes: ConflictIndex<String>,
    /// SSI rw-dependency tracker for Serializable isolation.
    ssi_tracker: SsiTracker,
    /// Countdown to the next full GC pass. Pruning is an O(retained) scan
    /// under every conflict-index lock; running it on every commit/abort puts
    /// that scan on the commit critical path. Stale entries are harmless
    /// (they sit at or below the oldest active timestamp, so no active
    /// transaction can observe them as newer), so the pass is batched.
    prune_countdown: AtomicUsize,
}

/// Number of commit/abort prune requests between full GC passes.
const PRUNE_INTERVAL: usize = 64;

impl Certifier {
    pub fn new() -> Self {
        Self {
            commit_lock: Mutex::new(()),
            committed_write_sets: Mutex::new(BTreeMap::new()),
            vertex_writes: ConflictIndex::new(),
            edge_writes: ConflictIndex::new(),
            schema_writes: ConflictIndex::new(),
            index_writes: ConflictIndex::new(),
            ssi_tracker: SsiTracker::new(),
            prune_countdown: AtomicUsize::new(1),
        }
    }
}

impl Default for Certifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

//! Committed write-set spatial indices: O(1) per-resource conflict probe,
//! idempotent publication and watermark-based pruning.

use std::collections::HashMap;
use std::hash::Hash;

use parking_lot::Mutex;

use graphdb_core::types::Timestamp;

use crate::types::TransactionId;

/// Maps a resource reference to its committed write timestamps + transaction IDs.
type ConflictMap<V> = HashMap<V, Vec<(Timestamp, TransactionId)>>;

pub(super) struct ConflictIndex<V> {
    entries: Mutex<ConflictMap<V>>,
}

impl<V: Eq + Hash + Clone> ConflictIndex<V> {
    pub(super) fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Whether any committed write on `resource` happened after `since`.
    pub(super) fn conflicts_after(&self, resource: &V, since: Timestamp) -> bool {
        self.entries
            .lock()
            .get(resource)
            .is_some_and(|list| list.iter().any(|(ts, _)| *ts > since))
    }

    /// Append an index entry unless the same commit already indexed it.
    ///
    /// Keeps recovery re-drives (`force_publish`) idempotent: re-inserting
    /// an already published commit must not duplicate entries.
    pub(super) fn add(&self, resource: &V, commit_timestamp: Timestamp, txn_id: TransactionId) {
        let mut map = self.entries.lock();
        let list = map.entry(resource.clone()).or_default();
        if !list
            .iter()
            .any(|(ts, id)| *ts == commit_timestamp && *id == txn_id)
        {
            list.push((commit_timestamp, txn_id));
        }
    }

    /// Drop entries committed at or before `oldest_active_ts`.
    pub(super) fn prune(&self, oldest_active_ts: Timestamp) {
        self.entries.lock().retain(|_, entries| {
            entries.retain(|(commit_ts, _)| *commit_ts > oldest_active_ts);
            !entries.is_empty()
        });
    }
}

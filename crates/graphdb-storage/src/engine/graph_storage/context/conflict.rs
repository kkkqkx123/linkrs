use std::collections::VecDeque;

use graphdb_core::types::Timestamp;
use graphdb_transaction::types::WriteSet;

/// Maximum retained committed write sets. Bounds certification memory while
/// covering recent concurrent commits.
const COMMITTED_WRITE_SET_CAPACITY: usize = 512;

/// Maximum entities of a single write set admitted into the window. Larger
/// bulk statements skip publishing (their own commit still certifies against
/// the window); this keeps a bulk load from evicting the whole OLTP history.
const MAX_PUBLISHED_WRITE_SET_SIZE: usize = 8192;

/// Recently committed write sets for auto-commit commit-time certification.
///
/// Auto-commit statements bypass the transaction manager, so its `Certifier`
/// never sees them. This window records their write sets at commit; each new
/// auto-commit certifies its local write set against entries committed after
/// its read timestamp via `WriteSet::has_conflict_with` and fails with a
/// write-write conflict instead of silently overwriting.
pub(crate) struct CommittedWriteSetWindow {
    entries: parking_lot::Mutex<VecDeque<(Timestamp, WriteSet)>>,
}

impl CommittedWriteSetWindow {
    pub(crate) fn new() -> Self {
        Self {
            entries: parking_lot::Mutex::new(VecDeque::new()),
        }
    }

    /// Whether `local` overlaps any committed write set newer than `read_ts`.
    pub(crate) fn has_conflict(&self, local: &WriteSet, read_ts: Timestamp) -> bool {
        if local.is_empty() {
            return false;
        }
        let entries = self.entries.lock();
        entries.iter().any(|(commit_ts, committed)| {
            *commit_ts > read_ts && local.has_conflict_with(committed)
        })
    }

    /// Record a committed write set and prune entries no active reader can
    /// overlap (`commit_ts <= horizon`) beyond the capacity bound.
    pub(crate) fn publish(&self, commit_ts: Timestamp, write_set: WriteSet, horizon: Timestamp) {
        if write_set.is_empty() || write_set.size() > MAX_PUBLISHED_WRITE_SET_SIZE {
            return;
        }
        let mut entries = self.entries.lock();
        entries.push_back((commit_ts, write_set));
        while entries.len() > COMMITTED_WRITE_SET_CAPACITY {
            entries.pop_front();
        }
        while entries.len() > 1
            && entries
                .front()
                .is_some_and(|(ts, _)| *ts <= horizon && horizon != u64::MAX)
        {
            entries.pop_front();
        }
    }
}

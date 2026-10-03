//! Read frontier advancement, expired write reaping and lifecycle resets.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use graphdb_core::types::Timestamp;

use super::types::WriteTimestampState;
use super::VersionManager;

impl VersionManager {
    pub(super) fn advance_read_frontier(
        &self,
        states: &mut BTreeMap<Timestamp, (Instant, WriteTimestampState)>,
    ) {
        let mut frontier = self.read_ts.load(Ordering::Acquire);
        loop {
            let next = frontier.saturating_add(1);
            match states.get(&next).map(|(_, state)| *state) {
                Some(WriteTimestampState::Committed | WriteTimestampState::Aborted) => {
                    frontier = next;
                    states.remove(&next);
                }
                _ => break,
            }
        }
        self.read_ts.store(frontier, Ordering::Release);
    }

    /// Abort `Pending` write timestamps older than `write_reap_timeout`,
    /// advancing the read frontier so version GC can proceed.
    ///
    /// This is a safety net for write timestamps whose owning path vanished
    /// (orphaned write). Callers must pass the set of timestamps currently
    /// owned by live write transactions so those are never reaped; reaping a
    /// live transaction's timestamp would silently discard its writes.
    ///
    /// Returns the number of timestamps reaped.
    pub fn reap_expired_write_timestamps(
        &self,
        timeout: Duration,
        owned: &HashSet<Timestamp>,
    ) -> usize {
        let now = Instant::now();
        let mut states = self.write_states.lock();
        let expired: Vec<Timestamp> = states
            .iter()
            .filter(|(ts, (acquired, state))| {
                *state == WriteTimestampState::Pending
                    && !owned.contains(ts)
                    && now.duration_since(*acquired) > timeout
            })
            .map(|(ts, _)| *ts)
            .collect();

        let mut reaped = 0;
        for ts in expired {
            if let Some((_, entry)) = states.get_mut(&ts) {
                if *entry == WriteTimestampState::Pending {
                    *entry = WriteTimestampState::Aborted;
                    let _ = self.snapshot_tracker.release_snapshot(ts);
                    self.write_pending.fetch_sub(1, Ordering::Relaxed);
                    reaped += 1;
                }
            }
        }

        if reaped > 0 {
            self.advance_read_frontier(&mut states);
        }
        drop(states);
        self.write_condvar.notify_all();
        reaped
    }

    pub fn init_ts(&self, ts: Timestamp) {
        // `write_ts` is the last allocated timestamp. Keeping the baseline at
        // the recovered timestamp makes the next allocation checked and
        // contiguous, including at the u64 boundary.
        self.write_ts.store(ts, Ordering::Release);
        self.read_ts.store(ts, Ordering::Release);
        self.write_states.lock().clear();
        // Restart rebuild: pins held by the previous process are gone with
        // it, so the tracker must restart empty. Otherwise the minimum
        // stays behind and GC stalls forever.
        self.snapshot_tracker.clear();
    }

    pub fn clear(&self) {
        // Preserve write_ts so that subsequent writes and checkpoints
        // use timestamps >= the compact timestamp, ensuring persisted
        // data remains visible after reload.
        self.read_ts.store(0, Ordering::Release);
        self.read_pending.store(0, Ordering::Relaxed);
        self.write_pending.store(0, Ordering::Relaxed);
        self.write_states.lock().clear();
        // The counters above are meaningless while stale pins survive:
        // drain the tracker as well so the safe-GC waterfront is not
        // pinned by snapshots that no longer exist.
        self.snapshot_tracker.clear();
    }

    #[cfg(test)]
    pub(super) fn backdate_write_timestamp(&self, ts: Timestamp, elapsed: Duration) {
        if let Some((acquired, _)) = self.write_states.lock().get_mut(&ts) {
            *acquired = Instant::now() - elapsed;
        }
    }
}

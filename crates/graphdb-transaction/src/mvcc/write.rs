//! Write lifecycle and commit ordering: slot state machine, commit
//! reservation and publication, and slot observability.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use graphdb_core::types::Timestamp;

use super::types::{TimestampSlot, VersionManagerError, VersionManagerResult, WriteTimestampState};
use super::VersionManager;

impl VersionManager {
    pub fn write_timestamp(&self) -> Timestamp {
        self.write_ts.load(Ordering::Acquire)
    }

    /// Allocate the next write timestamp.
    pub fn next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        self.try_next_write_timestamp()
    }

    pub fn try_next_write_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let ts = self.reserve_timestamp()?;
        self.write_pending.fetch_add(1, Ordering::Relaxed);
        self.snapshot_tracker
            .add_snapshot(ts)
            .map_err(|_| VersionManagerError::SnapshotTrackingFailed)?;
        self.write_states
            .lock()
            .insert(ts, (Instant::now(), WriteTimestampState::Pending));
        Ok(ts)
    }

    pub fn acquire_insert_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let ts = self.reserve_timestamp()?;
        if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
            log::error!("Failed to pre-reserve snapshot {}: {}", ts, e);
            return Err(VersionManagerError::SnapshotTrackingFailed);
        }
        self.write_states
            .lock()
            .insert(ts, (Instant::now(), WriteTimestampState::Pending));
        self.write_pending.fetch_add(1, Ordering::Relaxed);
        drop(_guard);
        Ok(ts)
    }

    pub fn abort_write_timestamp(&self, ts: Timestamp) {
        // Retires a timestamp that will never become visible. Aborting a
        // user transaction must go through the manager abort protocol so
        // SSI locks, leases and undo logs are released together with the
        // timestamp.
        self.finish_write_timestamp(ts, WriteTimestampState::Aborted);
    }

    /// Settle a system write timestamp in commit order.
    ///
    /// Reserves a commit timestamp for `start` and publishes visibility
    /// over both slots, so system commits share the commit-ordered
    /// coordinate with explicit transactions. Fails closed when the start
    /// slot is not a live pending write; callers must propagate the error
    /// instead of falling back to start-ordered publishing.
    pub fn commit_ordered(&self, start: Timestamp) -> VersionManagerResult<Timestamp> {
        let commit_ts = self.reserve_commit_timestamp(start)?;
        self.publish_reserved_commit(start, commit_ts);
        Ok(commit_ts)
    }

    /// Allocate a commit timestamp at commit time and retire the start slot.
    ///
    /// This restores Ladybug semantics (`commitTS = ++lastTimestamp`): read
    /// visibility is ordered by commit time, not by transaction start time.
    /// The start slot is retired as `Committed` (releasing its snapshot and
    /// pending count) and the freshly allocated commit timestamp is inserted
    /// as `Committed`, so the read frontier advances over both in one step.
    /// Out-of-order commits therefore never pin the frontier behind a
    /// still-pending start timestamp: every allocated timestamp reaches a
    /// terminal state exactly once, at commit time.
    ///
    /// Must only be called after the commit is durable (WAL) and storage
    /// finalization succeeded; otherwise unfinalized writes would become
    /// visible to new readers.
    ///
    /// Prefer the split `reserve_commit_timestamp` /
    /// `publish_reserved_commit` pair for new commit paths: reserving
    /// before finalization lets conflict certification index the exact
    /// commit timestamp while keeping visibility gated on finalization.
    /// This combined helper stays for recovery re-drives and tests.
    pub fn allocate_commit_timestamp(
        &self,
        start_ts: Timestamp,
    ) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let commit_ts = self.reserve_timestamp()?;
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&start_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
                let _ = self.snapshot_tracker.release_snapshot(start_ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
            }
        }
        states.insert(commit_ts, (Instant::now(), WriteTimestampState::Committed));
        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
        Ok(commit_ts)
    }

    /// Reserve a commit timestamp before storage finalization.
    ///
    /// The reserved timestamp is `Pending`, so the read frontier cannot
    /// cross it: visibility stays gated even though the exact commit
    /// timestamp is already known to conflict certification. The caller
    /// must settle the reservation exactly once, either with
    /// `publish_reserved_commit` on success or with
    /// `abort_write_timestamp` on failure (both are idempotent by slot
    /// state, so double-settling is safe).
    ///
    /// Refuses when the start slot is not a live `Pending` write: ordering
    /// visibility against a vanished owner would publish ownerless writes.
    pub fn reserve_commit_timestamp(&self, start_ts: Timestamp) -> VersionManagerResult<Timestamp> {
        let _guard = self.write_lock.lock();
        let mut states = self.write_states.lock();
        match states.get(&start_ts).map(|(_, state)| *state) {
            Some(WriteTimestampState::Pending) => {}
            _ => return Err(VersionManagerError::InvalidTimestamp(start_ts)),
        }
        let commit_ts = self.reserve_timestamp()?;
        states.insert(commit_ts, (Instant::now(), WriteTimestampState::Pending));
        Ok(commit_ts)
    }

    /// Settle a reservation made by `reserve_commit_timestamp`.
    ///
    /// Retires the start slot and marks the reserved commit timestamp
    /// `Committed`, then advances the read frontier over both. Must only
    /// be called after WAL durability and storage finalization succeeded.
    /// Tolerates missing slots (restart re-drives) by treating them as
    /// already settled instead of failing the commit.
    pub fn publish_reserved_commit(&self, start_ts: Timestamp, commit_ts: Timestamp) {
        let _guard = self.write_lock.lock();
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&start_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
                let _ = self.snapshot_tracker.release_snapshot(start_ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
            }
        }
        if let Some((_, entry)) = states.get_mut(&commit_ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = WriteTimestampState::Committed;
            }
        } else {
            states.insert(commit_ts, (Instant::now(), WriteTimestampState::Committed));
        }
        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
    }

    pub(super) fn finish_write_timestamp(&self, ts: Timestamp, state: WriteTimestampState) {
        let mut states = self.write_states.lock();
        if let Some((_, entry)) = states.get_mut(&ts) {
            if *entry == WriteTimestampState::Pending {
                *entry = state;
                let _ = self.snapshot_tracker.release_snapshot(ts);
                self.write_pending.fetch_sub(1, Ordering::Relaxed);
            }
        }

        self.advance_read_frontier(&mut states);
        drop(states);
        self.write_condvar.notify_all();
    }

    /// Return the observable state of one write-timestamp slot.
    ///
    /// Read-only probe for pending-aware visibility: storage asks about the
    /// creation/deletion stamp it just read and hides stamps owned by foreign
    /// pending transactions. Absent slots report `Vanished`; they were either
    /// reclaimed after the read frontier swallowed a run of terminal slots
    /// (their data is undone or committed and the plain predicate applies) or
    /// never owned a write slot at all.
    pub fn timestamp_slot(&self, ts: Timestamp) -> TimestampSlot {
        match self.write_states.lock().get(&ts).map(|(_, state)| *state) {
            Some(WriteTimestampState::Pending) => TimestampSlot::Pending,
            Some(WriteTimestampState::Committed) => TimestampSlot::Committed,
            Some(WriteTimestampState::Aborted) => TimestampSlot::Aborted,
            None => TimestampSlot::Vanished,
        }
    }

    /// Ages of all currently `Pending` write timestamps.
    ///
    /// Lets the cleanup path tell a live long-running transaction (its
    /// timestamp appears in the caller-supplied `owned` set and must never be
    /// reaped, but it still pins the read frontier) apart from a crash
    /// orphan (unowned and eligible for reaping). Sorted oldest-first so the
    /// worst frontier blocker is reported first.
    pub fn pending_write_ages(&self) -> Vec<(Timestamp, Duration)> {
        let now = Instant::now();
        let states = self.write_states.lock();
        let mut ages: Vec<(Timestamp, Duration)> = states
            .iter()
            .filter(|(_, (_, state))| *state == WriteTimestampState::Pending)
            .map(|(ts, (acquired, _))| (*ts, now.duration_since(*acquired)))
            .collect();
        ages.sort_by_key(|(_, age)| *age);
        ages.reverse();
        ages
    }
}

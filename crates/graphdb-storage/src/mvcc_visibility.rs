//! Unified MVCC visibility helper.
//!
//! Centralizes the `is_visible(snapshot, create_ts, delete_ts)` check that was
//! previously duplicated across column, edge and CSR layers. The helper
//! enforces timestamp-based visibility: callers pass the effective snapshot
//! timestamp, and transaction layers are responsible for advancing it only to
//! committed timestamps.

use graphdb_core::types::Timestamp;
use graphdb_transaction::{TimestampSlot, VersionManager};

/// Unified visibility check for MVCC version chains, vertex/edge tombstones and
/// CSR row visibility.
///
/// `snapshot` is the transaction's effective read timestamp (for the current
/// statement). `create_ts` is the version's creation/commit timestamp.
/// `delete_ts` is `None` for live rows and `Some(ts)` for tombstoned rows
/// (where `ts` is the deletion timestamp; rows are visible while
/// `snapshot < delete_ts`).
pub struct Visibility;

impl Visibility {
    #[inline]
    pub fn is_visible(
        snapshot: Timestamp,
        create_ts: Timestamp,
        delete_ts: Option<Timestamp>,
    ) -> bool {
        if create_ts > snapshot {
            return false;
        }
        match delete_ts {
            Some(del) => snapshot < del,
            None => true,
        }
    }

    #[inline]
    pub fn is_column_visible(snapshot: Timestamp, create_ts: Timestamp) -> bool {
        create_ts <= snapshot
    }

    #[inline]
    pub fn is_edge_visible(
        snapshot: Timestamp,
        create_ts: Timestamp,
        delete_ts: Timestamp,
    ) -> bool {
        create_ts <= snapshot && snapshot < delete_ts
    }

    /// Check visibility for a version-chain interval `[start_ts, end_ts)`.
    #[inline]
    pub fn is_version_visible(snapshot: Timestamp, start_ts: Timestamp, end_ts: Timestamp) -> bool {
        start_ts <= snapshot && snapshot < end_ts
    }

    /// Whether a version ending at `end_ts` may be reclaimed under GC cutoff `safe`.
    ///
    /// Dual of `is_visible`: a version is visible to some snapshot `snap >= end_ts`
    /// only while `snap < end_ts`, so once `end_ts <= safe` and every active
    /// snapshot satisfies `snap >= safe`, no active reader can observe it.
    /// All GC paths (column version chains, edge tombstones, CSR slots) share
    /// this exclusive-waterfront predicate so tombstone and slot reclamation
    /// cannot drift apart by one round.
    #[inline]
    pub fn is_gc_eligible(end_ts: Timestamp, safe: Timestamp) -> bool {
        end_ts <= safe
    }
}

/// Pending-aware visibility gate for operation-layer point lookups.
///
/// The plain [`Visibility`] predicates compare timestamps only, so an
/// optimistic write transaction running behind a concurrent uncommitted write
/// would observe foreign uncommitted rows (dirty read). This gate adds the
/// slot-state check with a lock-free fast path, evaluated per stamp in
/// priority order:
///
/// 1. stamp greater than the snapshot is invisible;
/// 2. stamp not greater than the live read frontier is trusted as committed
///    (single atomic load, no slot-map lock; zero overhead when no write is
///    in flight because every live stamp is below the frontier);
/// 3. stamp equal to the reader's own write timestamp is visible (a
///    transaction always observes its own writes: its read stamp is pinned
///    at or past its write stamp);
/// 4. otherwise the slot state decides: committed or vanished stamps are
///    trusted, pending or aborted stamps are hidden. Vanished slots were
///    reclaimed after the frontier swallowed a run of terminal slots, or
///    never owned a write slot, so the plain predicate applies.
///
/// Concrete types only, no dynamic dispatch. Callers keep the original
/// timestamp predicate for the first read and use this gate only to recheck
/// the surviving stamps of the record they already fetched.
pub struct PendingGate<'a> {
    manager: &'a VersionManager,
    own_write: Option<Timestamp>,
}

impl<'a> PendingGate<'a> {
    pub fn new(manager: &'a VersionManager, own_write: Option<Timestamp>) -> Self {
        Self { manager, own_write }
    }

    /// Whether `stamp` names a foreign uncommitted write at `snapshot`.
    ///
    /// True only for stamps inside `(frontier, snapshot]` that are neither
    /// the reader's own write nor committed/vanished. Column-chain fallback
    /// and delete-ignoring both key off this probe.
    pub fn is_foreign_pending(&self, snapshot: Timestamp, stamp: Timestamp) -> bool {
        if stamp > snapshot {
            return false;
        }
        if stamp <= self.manager.read_timestamp() {
            return false;
        }
        if Some(stamp) == self.own_write {
            return false;
        }
        matches!(
            self.manager.timestamp_slot(stamp),
            TimestampSlot::Pending | TimestampSlot::Aborted
        )
    }

    /// Whether a creation stamp is visible at `snapshot`.
    pub fn is_create_visible(&self, snapshot: Timestamp, create_ts: Timestamp) -> bool {
        if create_ts > snapshot {
            return false;
        }
        if create_ts <= self.manager.read_timestamp() {
            return true;
        }
        if Some(create_ts) == self.own_write {
            return true;
        }
        matches!(
            self.manager.timestamp_slot(create_ts),
            TimestampSlot::Committed | TimestampSlot::Vanished
        )
    }

    /// Effective deletion stamp: `None` means no deletion hides the row.
    ///
    /// A deletion at or below the snapshot hides the row only when it is
    /// trusted (below the frontier), the reader's own, or committed. A
    /// foreign pending or aborted deletion is ignored so the pre-delete row
    /// stays visible.
    pub fn effective_delete(
        &self,
        snapshot: Timestamp,
        delete_ts: Option<Timestamp>,
    ) -> Option<Timestamp> {
        let delete_ts = delete_ts?;
        if snapshot < delete_ts {
            return None;
        }
        if delete_ts <= self.manager.read_timestamp() {
            return Some(delete_ts);
        }
        if Some(delete_ts) == self.own_write {
            return Some(delete_ts);
        }
        match self.manager.timestamp_slot(delete_ts) {
            TimestampSlot::Committed | TimestampSlot::Vanished => Some(delete_ts),
            TimestampSlot::Pending | TimestampSlot::Aborted => None,
        }
    }

    /// Row liveness under pending awareness.
    pub fn is_row_visible(
        &self,
        snapshot: Timestamp,
        create_ts: Timestamp,
        delete_ts: Option<Timestamp>,
    ) -> bool {
        if !self.is_create_visible(snapshot, create_ts) {
            return false;
        }
        self.effective_delete(snapshot, delete_ts).is_none()
    }

    /// Edge liveness under pending awareness (`Timestamp::MAX` = live).
    pub fn is_edge_visible(
        &self,
        snapshot: Timestamp,
        create_ts: Timestamp,
        delete_ts: Timestamp,
    ) -> bool {
        let delete = if delete_ts == Timestamp::MAX {
            None
        } else {
            Some(delete_ts)
        };
        self.is_row_visible(snapshot, create_ts, delete)
    }
}

#[cfg(test)]
mod tests {
    use super::Visibility;

    #[test]
    fn visible_when_created_before_snapshot() {
        assert!(Visibility::is_visible(10, 5, None));
        assert!(!Visibility::is_visible(4, 5, None));
    }

    #[test]
    fn not_visible_when_deleted() {
        assert!(!Visibility::is_visible(10, 5, Some(10)));
        assert!(Visibility::is_visible(9, 5, Some(10)));
    }

    #[test]
    fn column_visible() {
        assert!(Visibility::is_column_visible(10, 10));
        assert!(!Visibility::is_column_visible(9, 10));
    }

    #[test]
    fn version_interval() {
        assert!(Visibility::is_version_visible(5, 5, 10));
        assert!(!Visibility::is_version_visible(10, 5, 10));
        assert!(!Visibility::is_version_visible(4, 5, 10));
    }

    #[test]
    fn gc_eligibility_matches_visibility_dual() {
        // end == safe: invisible to every snapshot >= safe, hence reclaimable.
        assert!(Visibility::is_gc_eligible(10, 10));
        assert!(Visibility::is_gc_eligible(9, 10));
        assert!(!Visibility::is_gc_eligible(11, 10));
        // Dual check: an eligible end is invisible to the boundary snapshot.
        assert!(!Visibility::is_visible(10, 5, Some(10)));
    }

    #[test]
    fn gate_hides_foreign_pending_create_but_keeps_own_write() {
        use super::PendingGate;
        use graphdb_transaction::VersionManager;
        let vm = VersionManager::new();
        let own = vm.acquire_insert_timestamp().expect("own write");
        let foreign = vm.acquire_insert_timestamp().expect("foreign write");
        let snapshot = foreign;

        // Foreign pending creation is invisible to a concurrent writer whose
        // snapshot covers the stamp.
        let foreign_view = PendingGate::new(&vm, Some(own));
        assert!(!foreign_view.is_create_visible(snapshot, foreign));
        assert!(!foreign_view.is_row_visible(snapshot, foreign, None));
        assert!(foreign_view.is_foreign_pending(snapshot, foreign));

        // Own write stays visible through the timestamp mechanism.
        assert!(foreign_view.is_create_visible(snapshot, own));
        assert!(foreign_view.is_row_visible(snapshot, own, None));
        assert!(!foreign_view.is_foreign_pending(snapshot, own));

        // After the foreign write commits, a fresh snapshot observes it.
        vm.commit_ordered(foreign).expect("ordered commit");
        vm.commit_ordered(own).expect("ordered commit");
        let later = PendingGate::new(&vm, None);
        assert!(later.is_create_visible(snapshot, foreign));
        assert!(!later.is_foreign_pending(snapshot, foreign));
    }

    #[test]
    fn gate_ignores_foreign_pending_delete() {
        use super::PendingGate;
        use graphdb_transaction::VersionManager;
        let vm = VersionManager::new();
        let create = vm.acquire_insert_timestamp().expect("create");
        vm.commit_ordered(create).expect("ordered commit");
        let delete = vm.acquire_insert_timestamp().expect("delete");
        let snapshot = delete;

        // The deleter is still pending: the row stays visible, the deletion
        // has no effective stamp.
        let gate = PendingGate::new(&vm, None);
        assert_eq!(gate.effective_delete(snapshot, Some(delete)), None);
        assert!(gate.is_row_visible(snapshot, create, Some(delete)));

        // Aborted deletions are reverted physically by undo, so the gate no
        // longer sees their stamps; once the frontier swallows the settled
        // slot the plain predicate applies again.
        vm.abort_write_timestamp(delete);
        assert!(!gate.is_foreign_pending(snapshot, delete));
    }

    #[test]
    fn gate_trusts_frontier_and_vanished_slots() {
        use super::PendingGate;
        use graphdb_transaction::VersionManager;
        let vm = VersionManager::new();
        let committed = vm.acquire_insert_timestamp().expect("write");
        vm.commit_ordered(committed).expect("ordered commit");
        // The frontier swallowed the terminal slot: the gate trusts the
        // plain predicate without consulting slot state.
        let gate = PendingGate::new(&vm, None);
        assert!(gate.is_create_visible(committed, committed));
        assert!(!gate.is_create_visible(committed - 1, committed));
        assert!(!gate.is_foreign_pending(committed, committed));
    }
}

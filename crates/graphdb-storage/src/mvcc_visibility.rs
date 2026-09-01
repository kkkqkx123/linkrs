//! Unified MVCC visibility helper.
//!
//! Centralizes the `is_visible(snapshot, create_ts, delete_ts)` check that was
//! previously duplicated across column, edge and CSR layers. The helper
//! enforces the Phase-4 invariant that uncommitted writes are only visible to
//! their owning transaction; other transactions observe the read frontier
//! captured in `snapshot`.

use graphdb_core::types::Timestamp;

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
}

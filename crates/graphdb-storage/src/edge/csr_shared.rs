//! Shared helpers for the CSR edge stores.
//!
//! `MutableCsr` (multi-edge rows) and `SingleMutableCsr` (one slot per
//! vertex) use different layouts but share the same vertex-capacity growth
//! policy and the same per-slot MVCC state machine (delete conflict, revert
//! window, tombstone GC eligibility). This module is the single source of
//! truth so the two stores cannot drift apart.

use super::{EdgeId, Nbr, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult};

pub(crate) const DEFAULT_VERTEX_CAPACITY: usize = 1024;
pub(crate) const VERTEX_GROWTH_FACTOR: f64 = 1.25;

/// Vertex-capacity growth shared by the CSR stores.
pub(crate) fn grown_vertex_capacity(min_capacity: usize) -> usize {
    ((min_capacity as f64 * VERTEX_GROWTH_FACTOR).ceil() as usize).max(min_capacity)
}

/// Decode a full-match endpoint key into the packed `(endpoint, rank)` pair
/// stored in `Nbr` rows. `decode_edge_endpoint` always yields an
/// int64-encoded vertex id, so `as_int64` is the canonical projection; the
/// low 32 bits match the former `as_u64` call sites bit for bit.
pub(crate) fn decode_endpoint_pair(dst: VertexId) -> (u32, i64) {
    let (vid, rank) = dst.decode_edge_endpoint();
    (vid.as_int64().unwrap_or(0) as u32, rank)
}

/// Outcome of running the shared per-slot delete state machine on an
/// edge-id-matched slot.
pub(crate) enum DeleteSlotOutcome {
    /// Caller must stamp `delete_ts` and account the removal.
    Stamped,
    /// Idempotent re-delete at the same timestamp; report `false`.
    AlreadyStamped,
    /// Edge not yet created at `ts`; report `false`.
    NotYetCreated,
}

/// Shared delete state machine for one matched slot: a tombstone stamped at
/// another timestamp is a write-write conflict, a tombstone at the same
/// timestamp is idempotent, and an edge created after `ts` is not deletable.
pub(crate) fn decide_slot_delete(
    nbr: &Nbr,
    edge_id: EdgeId,
    ts: Timestamp,
) -> StorageResult<DeleteSlotOutcome> {
    if nbr.delete_ts != Timestamp::MAX {
        if nbr.delete_ts != ts {
            return Err(StorageError::write_write_conflict(format!(
                "edge {:?} already deleted at ts={}, attempted delete at ts={}",
                edge_id, nbr.delete_ts, ts
            )));
        }
        return Ok(DeleteSlotOutcome::AlreadyStamped);
    }
    if nbr.create_ts > ts {
        return Ok(DeleteSlotOutcome::NotYetCreated);
    }
    Ok(DeleteSlotOutcome::Stamped)
}

/// Shared revert window: only deletions at or before the rollback point may
/// be undone.
pub(crate) fn can_revert_delete(nbr: &Nbr, ts: Timestamp) -> bool {
    nbr.delete_ts != Timestamp::MAX && nbr.delete_ts <= ts
}

/// Shared tombstone-eligibility predicate for one slot. Callers keep their
/// own `cutoff == Timestamp::MAX` early-out: without it every tombstone
/// would be eligible since `is_gc_eligible(ts, MAX)` always holds.
pub(crate) fn is_reclaimable_slot(nbr: &Nbr, cutoff: Timestamp) -> bool {
    nbr.delete_ts != Timestamp::MAX
        && crate::mvcc_visibility::Visibility::is_gc_eligible(nbr.delete_ts, cutoff)
}

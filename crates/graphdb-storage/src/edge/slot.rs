//! CSR slot primitives: the hot/cold neighbor split and its assembly.

use graphdb_core::types::INVALID_EDGE_ID;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

/// Hot topology half of a CSR slot: everything a traversal needs.
///
/// Packed `(endpoint, rank, edge_id)` without timestamp replicas. Scans that
/// resolve visibility through the version authority (by `edge_id`) walk this
/// half only, so the cold timestamp lines stay out of the cache. Assembled
/// back into [`Nbr`] at API boundaries through [`Nbr::from_parts`].
///
/// Rank keeps full 64-bit width: it is the caller-controlled multigraph
/// multiplicity key, shared with endpoint key packing, WAL redo records and
/// the query layer, so the store must hold any legal input value exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotNbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
}

/// Cold timestamp half of a CSR slot: physical MVCC replica.
///
/// Only `delete_ts` is stored inline (`Timestamp::MAX` means alive).
/// `create_ts` lives in the `EdgeTimestamps` authority and is consulted
/// on-demand for visibility. This keeps the cold half at 8 bytes. Query
/// paths must never decide visibility from this field alone; the version
/// authority owns that decision. Touched only by writes, deletes, rollback,
/// compaction and persistence assembly, never by topology scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdStamps {
    pub delete_ts: Timestamp,
}

impl HotNbr {
    /// Gap-fill sentinel matching [`Nbr::dead_gap`]: unassignable edge id.
    pub fn dead_gap() -> Self {
        Self {
            endpoint: 0,
            rank: 0,
            edge_id: INVALID_EDGE_ID,
        }
    }
}

impl ColdStamps {
    /// Gap-fill sentinel matching [`Nbr::dead_gap`]: empty stamp window.
    pub fn dead_gap() -> Self {
        Self { delete_ts: 0 }
    }

    /// Whether the slot holds no deletion stamp.
    #[inline]
    pub fn is_live(&self) -> bool {
        self.delete_ts == Timestamp::MAX
    }
}

/// Compact CSR edge entry.
///
/// Logical assembly of a [`HotNbr`] topology half and a [`ColdStamps`]
/// timestamp half. Storage layers persist the halves separately and
/// reassemble this record at API boundaries; the exact in-memory size
/// follows the measured struct size.
///
/// The `endpoint` is the internal vertex ID of the neighbor. The `rank` is the
/// edge multiplicity index (typically 0 for simple edges).
///
/// `delete_ts` is the deletion timestamp (`Timestamp::MAX` means alive),
/// maintained as physical state for row-level reclaim decisions.
/// `create_ts` lives in the `EdgeTimestamps` authority and is consulted
/// on-demand for MVCC visibility; it is not stored inline.
///
/// Topology and properties are decoupled: the CSR entry carries only the
/// topology (endpoint, rank, edge_id, timestamps). Edge properties are
/// stored in a separate columnar store indexed by `EdgeId` — no
/// `prop_offset` indirection is stored per edge.
///
/// Use [`Nbr::to_vertex_id`] to reconstruct the full `VertexId` at the API boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
    pub delete_ts: Timestamp,
}

impl Nbr {
    /// Create a new alive edge (delete_ts = MAX means "not deleted").
    pub fn new(endpoint: u32, rank: i64, edge_id: EdgeId) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts: Timestamp::MAX,
        }
    }

    /// Create with explicit create timestamp (stored in the authority, not inline).
    ///
    /// The returned `Nbr` carries only the topology and `delete_ts`; the
    /// caller must record `create_ts` in the `EdgeTimestamps` authority.
    pub fn with_create_ts(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        _create_ts: Timestamp,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts: Timestamp::MAX,
        }
    }

    /// Create with explicit delete timestamp.
    pub fn with_timestamps(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        delete_ts: Timestamp,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts,
        }
    }

    /// Gap-fill sentinel for rebuilt rows: never alive at any timestamp.
    ///
    /// Uses the unassignable edge id with an empty delete window,
    /// so an overrun scan reports absence instead of a ghost live edge. This
    /// is the only sanctioned filler for reserved primary slots; every CSR
    /// shape shares it.
    pub fn dead_gap() -> Self {
        Self {
            endpoint: 0,
            rank: 0,
            edge_id: INVALID_EDGE_ID,
            delete_ts: 0,
        }
    }

    /// Check if this edge is physically reclaimable at the given cutoff.
    ///
    /// Physical reclaim probe only: compares the projected `delete_ts` replica
    /// without the authority `create_ts`. Query visibility must go through the
    /// version authority
    /// ([`crate::mvcc_visibility::Visibility::is_edge_visible`]); using this
    /// probe for queries would fork a second visibility decision that drifts
    /// from the authority. Visible to the edge subtree only so storage layers
    /// outside edge storage cannot mistake it for a visibility check.
    #[inline]
    pub(in crate::edge) fn is_alive_at(&self, ts: Timestamp) -> bool {
        ts < self.delete_ts
    }

    /// Split the record into its hot topology half.
    #[inline]
    pub fn hot(&self) -> HotNbr {
        HotNbr {
            endpoint: self.endpoint,
            rank: self.rank,
            edge_id: self.edge_id,
        }
    }

    /// Split the record into its cold timestamp half.
    #[inline]
    pub fn cold(&self) -> ColdStamps {
        ColdStamps {
            delete_ts: self.delete_ts,
        }
    }

    /// Reassemble a record from its halves.
    #[inline]
    pub fn from_parts(hot: HotNbr, cold: ColdStamps) -> Self {
        Self {
            endpoint: hot.endpoint,
            rank: hot.rank,
            edge_id: hot.edge_id,
            delete_ts: cold.delete_ts,
        }
    }

    /// Reconstruct the full `VertexId` from the packed `(endpoint, rank)` pair.
    ///
    /// The result is a 16-byte VertexId encoding `(endpoint as i64, rank)` in
    /// big-endian, matching the format produced by
    /// `EdgeTable::edge_endpoint_key`.
    #[inline]
    pub fn to_vertex_id(&self) -> VertexId {
        VertexId::edge_endpoint_key(self.endpoint, self.rank)
    }
}

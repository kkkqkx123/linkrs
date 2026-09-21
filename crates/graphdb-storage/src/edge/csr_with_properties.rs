//! CSR with Properties — ladybug-style columnar storage.
//!
//! Properties are stored in parallel column arrays keyed by `EdgeId` through
//! the edge-to-row map. The topology CSR owns the only adjacency index; this
//! store keeps no per-vertex offsets, lengths, or append heads.
//!
//! This implementation uses `Column` (continuous arrays) instead of
//! `HashMap<u32, Value>` for cache-friendly scans and lower memory overhead.
//!
//! Visibility authority lives in `MVCCManager` (`edge_timestamps`): these CSR
//! row stamps are only a physical projection kept in sync on the write path
//! for garbage collection. They must never decide query visibility alone.

use std::collections::{HashMap, HashSet};

use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::Value;

use crate::edge::property_schema::PropertySchema;
use crate::vertex::column::Column;

/// Row visibility for MVCC.
///
/// Physical projection of the version authority for collection only.
/// Every visibility decision delegates to the single
/// [`crate::mvcc_visibility::Visibility`] predicate so this copy can never
/// drift into a second time comparison.
#[derive(Debug, Clone, Copy)]
struct RowVisibility {
    create_ts: Timestamp,
    delete_ts: Option<Timestamp>,
}

/// Columnar property storage keyed by edge id.
///
/// Every edge owns exactly one row, including edges without properties.
/// Row identity is the edge-to-row map; there is no per-vertex addressing.
///
/// Memory and persistence semantics: property before-images (row version
/// chains) stay memory-only; `dump` serializes current values plus row
/// visibility, per-column stable identifiers, encoding choices and refreshed
/// statistics. A reload restores plain values first, then re-applies the
/// recorded encodings and statistics, so encoded scans survive checkpoints.
/// Attribute time travel is therefore valid within a checkpoint epoch.
/// Row slot holding no edge mapping inside the dense edge map.
const UNMAPPED_ROW: u32 = u32::MAX;

/// Exported property row: `(create_ts, delete_ts, per-column values)`.
pub type ExportedRow = (Timestamp, Option<Timestamp>, Vec<(String, Option<Value>)>);

#[derive(Debug, Clone)]
pub struct CsrWithProperties {
    property_schema: Vec<PropertySchema>,
    property_columns: Vec<Column>,
    /// Column position by name, rebuilt on every schema mutation so hot
    /// paths never scan the schema linearly.
    column_index: HashMap<String, usize>,
    /// Column position by stable identifier, rebuilt with the name index.
    /// Undo parameters keyed by id resolve through this instead of scanning.
    prop_id_index: HashMap<i32, usize>,
    visibility: Vec<RowVisibility>,
    /// Dense edge-to-row map indexed by the table-allocated edge id.
    /// Edge ids are monotonic per table, so direct indexing replaces the
    /// former hash lookup; unmapped ids hold `UNMAPPED_ROW`.
    edge_to_row: Vec<u32>,
    /// Live mapping count, maintained alongside the dense map.
    edge_map_len: usize,
    /// Reverse index for O(1) row-to-edge lookup. Authoritative with
    /// `edge_to_row`; rebuilt on load, never persisted separately.
    row_to_edge: Vec<Option<EdgeId>>,
    free_list: Vec<u32>,
    row_count: usize,
    /// Column positions mutated since the last stats refresh or checkpoint.
    /// Drives per-column stats refresh so clean columns never pay recompute.
    /// Positions shift on schema mutation; the schema-mutating methods remap
    /// this set together with the schema.
    dirty_columns: HashSet<usize>,
    /// Stable column identifier allocator. Never reused or reassigned so
    /// stored undo parameters keyed by id stay valid across column drops.
    next_prop_id: i32,
    /// Inline-form marker: the owning table stores its single scalar in the
    /// CSR value column, so this store keeps only the schema and the
    /// name/id indexes. Every row operation fails instead of forking a
    /// second property truth.
    inline: bool,
}

pub(crate) mod encoding;
pub(crate) mod maintenance;
pub(crate) mod mapping;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod schema;
pub(crate) mod transfer;
pub(crate) mod visibility;
pub(crate) mod write;

#[cfg(test)]
mod tests;

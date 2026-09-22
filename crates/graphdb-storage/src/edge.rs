//! Edge Storage Module
//!
//! Provides CSR (Compressed Sparse Row) based edge storage.
//!
//! ## Components
//!
//! - `MutableCsr`: Mutable CSR supporting dynamic edge operations
//! - `SingleMutableCsr`: Optimized mutable CSR for single-edge scenarios
//! - `ImmutableCsr`: Frozen packed CSR for read-mostly groups (explicit
//!   freeze/unfreeze only, writes rejected while frozen)
//! - `CsrVariant`: Enum wrapper for runtime CSR selection
//! - `CsrWithProperties`: Ladybug-style columnar property storage
//! - `CsrShardSet`: Node-group sharded topology container routing by endpoint interval
//! - `EdgeStore`: Node-group sharded edge table combining out/in shards and property storage
//!
//! ## CSR Type Selection
//!
//! The `EdgeStrategy` enum determines which CSR type to use:
//! - `Multiple`: Use `MutableCsr` (supports multiple edges per vertex)
//! - `Single`: Use `SingleMutableCsr` (one edge per vertex, O(1) access)
//! - `None`: No edges stored
//!
//! ## Use Cases
//!
//! | Strategy | CSR Type | Use Case | Time Complexity |
//! |----------|----------|----------|-----------------|
//! | `Multiple` | `MutableCsr` | General multi-edge relationships | O(degree) |
//! | `Single` | `SingleMutableCsr` | One-to-one relationships (spouse, current_employer) | O(1) |
//! | `None` | - | No edges stored | - |

pub(crate) mod bundled_csr;
pub(crate) mod csr_shared;
pub mod csr_trait;
pub mod csr_variant;
pub mod csr_with_properties;
pub mod edge_table;
pub mod fragmentation_stats;
pub mod immutable_csr;
pub mod mutable_csr;
pub mod node_group;
pub mod property_schema;
pub(crate) mod pure_csr;
pub mod single_mutable_csr;

use crate::types::StoragePropertyDef;
pub use csr_trait::{CsrBase, MutableCsrTrait};
pub use csr_variant::{CsrRowIter, CsrVariant};
pub use csr_with_properties::CsrWithProperties;
pub use edge_table::core::UpdateEdgePropertyByKeyParams;
pub use edge_table::{EdgeIndexStatus, EdgeStore, IncidentDeletedEdge};
pub use fragmentation_stats::{
    FragmentationStats, VertexFragmentation, GROUP_FRAGMENTATION_THRESHOLD,
};
pub use graphdb_core::types::EdgeStrategy;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, Value};
pub use mutable_csr::{EdgePosition, MutableCsr, MutableCsrIterator};
pub use node_group::{
    region_id_for_local, region_local_range, regions_per_group, CsrShardSet, EdgeCheckpointKind,
    FreezeBlockReason, FreezeFeasibility, GroupDirty, NodeGroupStats, RegionDirty,
    RegionMergeScope, ShardCsrIterator, TableShardManifest, DEFAULT_NODE_GROUP_BITS,
    GROUP_MERGE_MIN_DENSITY, LEAF_REGION_ROWS, REGION_MERGE_MIN_DENSITY,
};
pub use single_mutable_csr::{SingleMutableCsr, SingleMutableCsrIterator};

pub use bundled_csr::{decode_scalar, encode_scalar, BundledCsr};
pub use edge_table::checkpoint::snapshot::{
    MappedFrozen, MappedFrozenIterator, MappedFrozenRowIter,
};
pub use graphdb_core::types::INVALID_EDGE_ID;
pub use immutable_csr::{FrozenRowIter, ImmutableCsr, ImmutableCsrIterator};
pub use pure_csr::{PureAllIter, PureRowIter, PureTopologyCsr};

/// One edge batch-insert entry: `(src, dst, rank, properties, ts)`.
pub type BatchInsertEntry<'a> = (u32, u32, i64, &'a [(String, Value)], Timestamp);

/// Decoded neighbor for bulk puts: `(endpoint, rank, edge_id, ts)`.
pub type EdgePut = (u32, i64, EdgeId, Timestamp);

/// One source row's bulk-put batch: `(local_src, entries)`.
pub type RowEdgeBatch = (u32, Vec<EdgePut>);

/// Resolved record form for an edge table, persisted in `meta.bin`.
///
/// Determined once at table creation by the selector; never re-inferred on
/// load. The choice locks the physical layout: later property additions,
/// type changes or rank usage breaking the preconditions need an explicit
/// migration (`EdgeStore::migration_plan`, `migrate_record_form` or
/// `switch_record_form_online`) followed by a checkpoint, never an
/// in-place reinterpretation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum RecordForm {
    /// Pure topology: 12 bytes/edge, no rank, no timestamps.
    Pure,
    /// Bundled: 20 bytes/edge, inline single scalar value column.
    Bundled,
    /// Standard columnar property storage (default / fallback).
    #[default]
    Columnar,
}

/// User-facing preference for record form selection at table creation time.
///
/// `Auto` derives the form from the schema (no properties to pure, one
/// encodable scalar to bundled, otherwise columnar) and reports the result
/// through the table construction log; the resolved form then locks and
/// persists. Later evolution breaking the preconditions must migrate
/// explicitly. `Columnar` forces the standard multi/single/none strategy path.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum RecordFormPreference {
    /// Auto-select Pure/Bundled/Columnar based on schema properties.
    Auto,
    /// Force columnar storage regardless of schema.
    #[default]
    Columnar,
}

/// Shared rejection for a single-edge strategy paired with an inline form.
///
/// Single directions need fixed single slots, which only the columnar form
/// provides. Every construction, load and migration gate reports this exact
/// wording so operators see one conflict and one way out.
pub(crate) const SINGLE_REQUIRES_COLUMNAR_MSG: &str =
    "single edge strategy requires the columnar record form; adjust the strategy or keep the columnar form, see migration_plan/migrate_record_form";

/// Check whether a `DataType` can be encoded as a 64-bit scalar for the
/// `Bundled` record form.
pub fn is_scalar_encodable(dt: &graphdb_core::DataType) -> bool {
    use graphdb_core::DataType;
    matches!(
        dt,
        DataType::Bool
            | DataType::SmallInt
            | DataType::Int
            | DataType::BigInt
            | DataType::Float
            | DataType::Double
            | DataType::Date
            | DataType::Time
            | DataType::DateTime
    )
}

/// Whether a schema may use the `Bundled` record form.
///
/// Single source of truth for the bundled admission rules, shared by table
/// creation and record-form migration: exactly one property, an encodable
/// scalar type, and no single-edge direction. Bundled additionally carries
/// no rank, no MVCC version chain and no freeze support for valid values,
/// so schemas expecting those must stay columnar even when eligible here.
pub fn is_bundled_eligible(
    properties: &[StoragePropertyDef],
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
) -> bool {
    bundled_ineligibility_reason(properties, oe_strategy, ie_strategy).is_none()
}

/// Why a schema cannot use the `Bundled` record form, if it cannot.
///
/// Returns the same wording the migration precheck reports, so creation and
/// migration refuse a bundled target for the same stated reason.
pub fn bundled_ineligibility_reason(
    properties: &[StoragePropertyDef],
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
) -> Option<String> {
    if oe_strategy == EdgeStrategy::Single || ie_strategy == EdgeStrategy::Single {
        return Some(SINGLE_REQUIRES_COLUMNAR_MSG.to_string());
    }
    if properties.len() != 1 {
        return Some(
            "bundled record form requires exactly one property; adjust the schema or keep the columnar form, see migration_plan/migrate_record_form"
                .to_string(),
        );
    }
    if !is_scalar_encodable(&properties[0].data_type) {
        return Some(format!(
            "property type {:?} cannot inline into the bundled form; keep the columnar form, see migration_plan/migrate_record_form",
            properties[0].data_type
        ));
    }
    None
}

#[derive(Debug, Clone)]
pub struct EdgeRecord {
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub rank: i64,
    pub properties: Vec<(String, Value)>,
}

impl From<&EdgeRecord> for Edge {
    fn from(record: &EdgeRecord) -> Self {
        let props: std::collections::HashMap<String, Value> =
            record.properties.iter().cloned().collect();

        Edge {
            src: record.src_vid,
            dst: record.dst_vid,
            edge_type: String::new(),
            ranking: record.rank,
            props,
        }
    }
}

impl EdgeRecord {
    pub fn into_edge_with_type(self, edge_type: &str) -> Edge {
        let props: std::collections::HashMap<String, Value> = self.properties.into_iter().collect();

        Edge {
            src: self.src_vid,
            dst: self.dst_vid,
            edge_type: edge_type.to_string(),
            ranking: self.rank,
            props,
        }
    }
}

/// Storage direction of an edge table, derived from the CSR strategies.
///
/// Both directions enabled pays double writes and double storage but serves
/// forward and reverse traversal from local rows. Single-direction tables
/// store only one leg and answer the missing direction as empty.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum StorageDirection {
    /// Outgoing and incoming legs stored.
    #[default]
    Both,
    /// Only the outgoing leg stored.
    OutOnly,
    /// Only the incoming leg stored.
    InOnly,
}

/// Cardinality contract of one direction, derived from the CSR strategy.
///
/// Single maps to one live edge per bound vertex, Multiple maps to many.
/// The topology guard already rejects a second live edge on Single slots;
/// this type makes the contract explicit for planning and error reporting.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum EdgeMultiplicity {
    /// At most one live edge per bound vertex.
    One,
    /// Any number of live edges per bound vertex.
    #[default]
    Many,
}

/// Consistency contract of the secondary property index.
///
/// Best-effort keeps the primary write authoritative and counts index
/// failures as lag; Strong fails the primary write when the index write
/// fails, for small-cardinality critical attributes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum IndexConsistency {
    /// Primary write succeeds, index lag counted and rebuilt later.
    #[default]
    BestEffort,
    /// Index failure fails the primary write.
    Strong,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,
    pub ie_strategy: EdgeStrategy,
    pub schema_version: u64,
    /// Persisted record form: how edges are stored on disk.
    /// Determined once at table creation by the selector; never re-inferred
    /// on load. Default is `Columnar`.
    #[serde(default)]
    pub record_form: RecordForm,
}

impl EdgeSchema {
    /// Validate that the schema has at least one enabled direction.
    /// Single-direction tables are supported: the write path stores only
    /// the enabled leg and reads on the missing leg report empty.
    ///
    /// The single-plus-inline combination is intentionally not checked here:
    /// table creation overwrites `record_form` from the configured
    /// preference, so the input value carries no authority. Resolved schemas
    /// are enforced at shard construction and on load instead.
    pub fn validate(&self) -> graphdb_core::StorageResult<()> {
        if self.oe_strategy == EdgeStrategy::None && self.ie_strategy == EdgeStrategy::None {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "EdgeSchema '{}': at least one of oe_strategy and ie_strategy must be enabled",
                self.label_name
            )));
        }
        Ok(())
    }

    /// Storage direction derived from the enabled CSR strategies.
    pub fn storage_direction(&self) -> StorageDirection {
        match (
            self.oe_strategy != EdgeStrategy::None,
            self.ie_strategy != EdgeStrategy::None,
        ) {
            (true, true) => StorageDirection::Both,
            (true, false) => StorageDirection::OutOnly,
            (false, true) => StorageDirection::InOnly,
            (false, false) => StorageDirection::Both,
        }
    }

    /// Whether the outgoing leg is stored.
    pub fn has_out(&self) -> bool {
        self.oe_strategy != EdgeStrategy::None
    }

    /// Whether the incoming leg is stored.
    pub fn has_in(&self) -> bool {
        self.ie_strategy != EdgeStrategy::None
    }

    /// Cardinality contract of the outgoing direction.
    pub fn out_multiplicity(&self) -> EdgeMultiplicity {
        if self.oe_strategy == EdgeStrategy::Single {
            EdgeMultiplicity::One
        } else {
            EdgeMultiplicity::Many
        }
    }

    /// Cardinality contract of the incoming direction.
    pub fn in_multiplicity(&self) -> EdgeMultiplicity {
        if self.ie_strategy == EdgeStrategy::Single {
            EdgeMultiplicity::One
        } else {
            EdgeMultiplicity::Many
        }
    }

    /// Validate schema at creation time
    /// Ensures property names are valid and edge types are well-formed
    pub fn validate_on_creation(&self) -> graphdb_core::StorageResult<()> {
        // Validate edge name
        if self.label_name.is_empty() {
            return Err(graphdb_core::StorageError::invalid_operation(
                "Edge type name cannot be empty".to_string(),
            ));
        }

        Self::validate_identifier_internal(&self.label_name)?;

        // Validate strategy compatibility
        self.validate()?;

        // Validate property names are unique and valid
        let mut seen_names = std::collections::HashSet::new();
        for prop in &self.properties {
            if !seen_names.insert(&prop.name) {
                return Err(graphdb_core::StorageError::invalid_operation(format!(
                    "Duplicate property name in edge type '{}': '{}'",
                    self.label_name, prop.name
                )));
            }

            // Validate property name format
            if prop.name.is_empty() {
                return Err(graphdb_core::StorageError::invalid_operation(format!(
                    "Property name cannot be empty in edge type '{}'",
                    self.label_name
                )));
            }

            Self::validate_identifier_internal(&prop.name)?;

            // Validate property data types are not Empty or Null
            Self::validate_property_type_internal(&prop.data_type, &prop.name)?;
        }

        Ok(())
    }

    /// Validate that an identifier (name) follows valid rules
    fn validate_identifier_internal(name: &str) -> graphdb_core::StorageResult<()> {
        let first_char = match name.chars().next() {
            Some(c) => c,
            None => {
                return Err(graphdb_core::StorageError::invalid_operation(
                    "Identifier cannot be empty".to_string(),
                ));
            }
        };

        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "Identifier '{}' must start with ASCII letter or underscore, got '{}'",
                name, first_char
            )));
        }

        for (i, c) in name.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(graphdb_core::StorageError::invalid_operation(format!(
                    "Identifier '{}' contains invalid character '{}' at position {}. \
                     Only ASCII letters, digits, and underscores are allowed.",
                    name, c, i
                )));
            }
        }

        Ok(())
    }

    /// Validate that a property data type is allowed
    fn validate_property_type_internal(
        data_type: &graphdb_core::DataType,
        prop_name: &str,
    ) -> graphdb_core::StorageResult<()> {
        use graphdb_core::DataType;

        match data_type {
            DataType::Empty => Err(graphdb_core::StorageError::invalid_operation(format!(
                "Property '{}' cannot have type Empty - properties must have valid types",
                prop_name
            ))),
            DataType::Null => Err(graphdb_core::StorageError::invalid_operation(format!(
                "Property '{}' cannot have type Null - use nullable=true instead",
                prop_name
            ))),
            _ => Ok(()),
        }
    }
}

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
    /// from the authority. Restricted to crate-internal maintenance paths.
    #[inline]
    pub(crate) fn is_alive_at(&self, ts: Timestamp) -> bool {
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

    /// Decode the full `VertexId` from this entry and return `(vertex_id, rank)`.
    #[inline]
    pub fn decode_endpoint(&self) -> (VertexId, i64) {
        (self.to_vertex_id(), self.rank)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_half_stays_within_single_cache_line_budget() {
        // Per-edge memory budget, pinned exactly: 24-byte hot half plus
        // 8-byte cold half is 32 bytes per edge with no padding waste.
        // Rank stays 64-bit because it is a caller-controlled multigraph
        // key shared with the query layer, WAL redo records and endpoint
        // key packing; narrowing it would reject legal inputs instead of
        // storing them. create_ts lives in the EdgeTimestamps authority,
        // not inline; only delete_ts is kept for row-level reclaim.
        assert_eq!(std::mem::size_of::<HotNbr>(), 24);
        assert_eq!(std::mem::size_of::<ColdStamps>(), 8);
        assert_eq!(std::mem::size_of::<Nbr>(), 32);
    }

    #[test]
    fn slot_halves_roundtrip_through_nbr() {
        let nbr = Nbr::with_create_ts(7, 2, EdgeId(9), 100);
        let assembled = Nbr::from_parts(nbr.hot(), nbr.cold());
        assert_eq!(assembled, nbr);
        assert_eq!(
            Nbr::from_parts(HotNbr::dead_gap(), ColdStamps::dead_gap()),
            Nbr::dead_gap()
        );
    }

    #[test]
    fn test_edge_schema_validation_both_none() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "invalid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::None,
            ie_strategy: EdgeStrategy::None,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one"));
    }

    #[test]
    fn test_edge_schema_validation_oe_only() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "valid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::None,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_ok());
        assert_eq!(schema.storage_direction(), StorageDirection::OutOnly);
        assert!(schema.has_out());
        assert!(!schema.has_in());
    }

    #[test]
    fn test_edge_schema_validation_ie_only() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "valid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::None,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_ok());
        assert_eq!(schema.storage_direction(), StorageDirection::InOnly);
        assert!(!schema.has_out());
        assert!(schema.has_in());
    }

    #[test]
    fn test_edge_schema_validation_both_enabled() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "valid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Single,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_ok());
    }
}

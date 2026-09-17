//! Edge Storage Module
//!
//! Provides CSR (Compressed Sparse Row) based edge storage.
//!
//! ## Components
//!
//! - `MutableCsr`: Mutable CSR supporting dynamic edge operations
//! - `SingleMutableCsr`: Optimized mutable CSR for single-edge scenarios
//! - `CsrVariant`: Enum wrapper for runtime CSR selection (mutable variants only)
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

pub(crate) mod csr_shared;
pub mod csr_trait;
pub mod csr_variant;
pub mod csr_with_properties;
pub mod edge_table;
pub mod fragmentation_stats;
pub mod mutable_csr;
pub mod node_group;
pub mod property_schema;
pub mod single_mutable_csr;

use crate::types::StoragePropertyDef;
pub use csr_trait::{CsrBase, MutableCsrTrait};
pub use csr_variant::CsrVariant;
pub use csr_with_properties::CsrWithProperties;
pub use edge_table::core::UpdateEdgePropertyByKeyParams;
pub use edge_table::EdgeStore;
pub use fragmentation_stats::{FragmentationStats, VertexFragmentation};
pub use graphdb_core::types::EdgeStrategy;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, Value};
pub use mutable_csr::{MutableCsr, MutableCsrIterator};
pub use node_group::{
    region_id_for_local, region_local_range, regions_per_group, CsrShardSet, EdgeCheckpointKind,
    GroupDirty, NodeGroupStats, RegionDirty, RegionMergeScope, ShardCsrIterator,
    TableShardManifest, DEFAULT_NODE_GROUP_BITS, GROUP_MANIFEST_VERSION, GROUP_MERGE_MIN_DENSITY,
    LEAF_REGION_ROWS, REGION_MERGE_MIN_DENSITY,
};
pub use single_mutable_csr::{SingleMutableCsr, SingleMutableCsrIterator};

pub use graphdb_core::types::INVALID_EDGE_ID;

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
}

impl EdgeSchema {
    /// Validate that the schema has compatible CSR strategies.
    /// Both directions must be enabled: the write path performs
    /// unconditional double writes, so a single-direction table would
    /// construct successfully yet fail every write on the disabled leg.
    pub fn validate(&self) -> graphdb_core::StorageResult<()> {
        if self.oe_strategy == EdgeStrategy::None || self.ie_strategy == EdgeStrategy::None {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "EdgeSchema '{}': oe_strategy and ie_strategy must both be enabled. \
                         Single-direction tables are not supported",
                self.label_name
            )));
        }
        Ok(())
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

/// Compact CSR edge entry.
///
/// Stores the neighbor as a packed `(endpoint: u32, rank: i64)` pair instead of a
/// full [`VertexId`] (33 bytes). This reduces per-edge overhead from 41 to 20 bytes
/// in the mutable CSR, and from 49 to 20 bytes in the immutable CSR.
///
/// The `endpoint` is the internal vertex ID of the neighbor. The `rank` is the
/// edge multiplicity index (typically 0 for simple edges).
///
/// `create_ts` is the creation timestamp kept as a physical replica for
/// compaction and debugging. `delete_ts` is the deletion timestamp
/// (`Timestamp::MAX` means alive), also maintained as physical state.
///
/// Visibility authority lives in `MVCCManager` (`edge_timestamps`): query
/// paths must decide visibility through `is_edge_visible`, never by reading
/// these row fields directly.
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
    pub create_ts: Timestamp,
    pub delete_ts: Timestamp,
}

impl Nbr {
    /// Create a new alive edge (delete_ts = MAX means "not deleted").
    pub fn new(endpoint: u32, rank: i64, edge_id: EdgeId) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            create_ts: 0,
            delete_ts: Timestamp::MAX,
        }
    }

    /// Create with explicit create timestamp.
    pub fn with_create_ts(endpoint: u32, rank: i64, edge_id: EdgeId, create_ts: Timestamp) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            create_ts,
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
            create_ts: 0,
            delete_ts,
        }
    }

    /// Check if this edge is alive at the given timestamp.
    /// An edge is alive when: create_ts <= ts AND ts < delete_ts.
    #[inline]
    pub fn is_alive_at(&self, ts: Timestamp) -> bool {
        self.create_ts <= ts && ts < self.delete_ts
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
        };

        let result = schema.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must both be enabled"));
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
        };

        let result = schema.validate();
        assert!(result.is_err());
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
        };

        let result = schema.validate();
        assert!(result.is_err());
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
        };

        let result = schema.validate();
        assert!(result.is_ok());
    }
}

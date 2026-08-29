//! Edge Storage Module
//!
//! Provides CSR (Compressed Sparse Row) based edge storage.
//!
//! ## Components
//!
//! - `MutableCsr`: Mutable CSR supporting dynamic edge operations
//! - `Csr`: Read-only immutable CSR for frozen segments and snapshots
//! - `SingleMutableCsr`: Optimized mutable CSR for single-edge scenarios
//! - `CsrVariant`: Enum wrapper for runtime CSR selection (mutable variants only)
//! - `EdgeTable`: Edge table combining out/in CSRs and property storage
//! - `PropertyTable`: Edge property storage
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

pub mod bloom_filter;
pub mod csr;
pub mod csr_trait;
pub mod csr_variant;
pub mod edge_table;
pub mod fragmentation_stats;
pub mod labeled_mutable_csr;
pub mod multi_single_mutable_csr;
pub mod mutable_csr;
pub mod property_schema;
pub mod property_table;
pub mod single_mutable_csr;

use property_schema::PROP_OFFSET_NONE;
use crate::types::StoragePropertyDef;
use graphdb_core::types::{EdgeId, LabelId, Timestamp, VertexId};
use graphdb_core::{Edge, Value};

pub use csr::Csr;
pub use csr_trait::{CsrBase, MutableCsrTrait};
pub use csr_variant::CsrVariant;
pub use edge_table::core::UpdateEdgePropertyByOffsetParams;
pub use edge_table::snapshot::ExportedEdgeSnapshot;
pub use edge_table::EdgeStore;
pub use fragmentation_stats::FragmentationStats;
pub use graphdb_core::types::EdgeStrategy;
pub use labeled_mutable_csr::{LabeledMutableCsr, LabeledMutableCsrIterator};
pub use multi_single_mutable_csr::{MultiSingleMutableCsr, MultiSingleMutableCsrIterator};
pub use mutable_csr::{MutableCsr, MutableCsrIterator};
pub use property_table::PropertyTable;
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
    /// At least one of out-edge or in-edge must be enabled (not None).
    pub fn validate(&self) -> graphdb_core::StorageResult<()> {
        if self.oe_strategy == EdgeStrategy::None && self.ie_strategy == EdgeStrategy::None {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "EdgeSchema '{}': both oe_strategy and ie_strategy are None. \
                         At least one direction must be enabled",
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
/// `prop_offset` is the PropertyTable row offset for this edge's properties.
/// Storing it here enables direct property access from CSR scan results
/// without the HashMap\<EdgeId, offset\> indirection (Phase 3 optimization).
/// A value of [`PROP_OFFSET_NONE`] (0) means no properties are associated.
///
/// Use [`Nbr::to_vertex_id`] to reconstruct the full `VertexId` at the API boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
    pub delete_ts: Timestamp,
    pub prop_offset: u32,
}

impl Nbr {
    /// Create a new alive edge (delete_ts = MAX means "not deleted").
    pub fn new(endpoint: u32, rank: i64, edge_id: EdgeId) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts: Timestamp::MAX,
            prop_offset: PROP_OFFSET_NONE,
        }
    }

    /// Create a new alive edge with an associated property offset.
    pub fn with_prop_offset(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        prop_offset: u32,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts: Timestamp::MAX,
            prop_offset,
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
            prop_offset: PROP_OFFSET_NONE,
        }
    }

    /// Create with explicit delete timestamp and property offset.
    pub fn with_timestamps_and_prop(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        delete_ts: Timestamp,
        prop_offset: u32,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            delete_ts,
            prop_offset,
        }
    }

    /// Check if this edge is alive at the given timestamp.
    /// An edge is alive when: create_ts <= ts AND ts < delete_ts.
    pub fn is_alive_at(&self, ts: Timestamp, create_ts: Timestamp) -> bool {
        create_ts <= ts && ts < self.delete_ts
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

/// Compact immutable CSR edge entry (frozen segments).
///
/// Like [`Nbr`], stores the neighbor as a packed `(endpoint: u32, rank: i64)`
/// pair. The `timestamp` field records the creation timestamp (used for
/// time-travel queries on frozen CSR data).
///
/// `prop_offset` is the PropertyTable row offset for this edge's properties,
/// enabling direct property access without HashMap indirection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImmutableNbr {
    pub endpoint: u32,
    pub rank: i64,
    pub edge_id: EdgeId,
    pub timestamp: Timestamp,
    pub prop_offset: u32,
}

impl ImmutableNbr {
    pub fn new(endpoint: u32, rank: i64, edge_id: EdgeId) -> Self {
        Self::with_timestamp(endpoint, rank, edge_id, 0)
    }

    pub fn with_timestamp(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            timestamp,
            prop_offset: PROP_OFFSET_NONE,
        }
    }

    /// Create with explicit property offset.
    pub fn with_timestamp_and_prop(
        endpoint: u32,
        rank: i64,
        edge_id: EdgeId,
        timestamp: Timestamp,
        prop_offset: u32,
    ) -> Self {
        Self {
            endpoint,
            rank,
            edge_id,
            timestamp,
            prop_offset,
        }
    }

    /// Reconstruct the full `VertexId` from the packed `(endpoint, rank)` pair.
    #[inline]
    pub fn to_vertex_id(&self) -> VertexId {
        VertexId::edge_endpoint_key(self.endpoint, self.rank)
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
            .contains("both oe_strategy and ie_strategy are None"));
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
        assert!(result.is_ok());
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
        assert!(result.is_ok());
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

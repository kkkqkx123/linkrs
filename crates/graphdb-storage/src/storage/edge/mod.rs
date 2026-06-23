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

pub mod csr;
pub mod csr_trait;
pub mod csr_variant;
#[path = "edge_table/mod.rs"]
pub mod edge_table;
pub mod fragmentation_stats;
pub mod labeled_mutable_csr;
pub mod mutable_csr;
pub mod multi_single_mutable_csr;
pub mod property_table;
pub mod property_schema;
pub mod single_mutable_csr;
pub mod bloom_filter;

use crate::core::types::{EdgeId, LabelId, Timestamp, VertexId, INVALID_TIMESTAMP};
use crate::core::{Edge, Value};
use crate::storage::types::StoragePropertyDef;

pub use crate::core::types::EdgeStrategy;
pub use csr::Csr;
pub use csr_trait::{CsrBase, MutableCsrTrait};
pub use csr_variant::CsrVariant;
pub use edge_table::{EdgeTable, ExportedEdgeSnapshot, UpdateEdgePropertyByOffsetParams, CompactionMode};
pub use fragmentation_stats::FragmentationStats;
pub use labeled_mutable_csr::{LabeledMutableCsr, LabeledMutableCsrIterator};
pub use mutable_csr::{MutableCsr, MutableCsrIterator};
pub use multi_single_mutable_csr::{MultiSingleMutableCsr, MultiSingleMutableCsrIterator};
pub use property_table::PropertyTable;
pub use property_schema::{PropertySchema, PropertyRecord, PropertyCompactionStats};
pub use single_mutable_csr::{SingleMutableCsr, SingleMutableCsrIterator};

pub use crate::core::types::INVALID_EDGE_ID;

/// Default schema version (1) for new schemas and deserialization
fn default_schema_version() -> u64 {
    1
}

#[derive(Debug, Clone, Copy)]
pub struct CompactionReport {
    /// Number of deleted edges that were removed
    pub removed_edges: usize,
    /// Number of bytes reclaimed
    pub reclaimed_bytes: usize,
    /// Fragmentation ratio before compaction
    pub old_fragmentation_ratio: f32,
    /// Fragmentation ratio after compaction
    pub new_fragmentation_ratio: f32,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeSchema {
    pub label_id: LabelId,
    pub label_name: String,
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub properties: Vec<StoragePropertyDef>,
    pub oe_strategy: EdgeStrategy,
    pub ie_strategy: EdgeStrategy,
    /// Schema version for migration tracking
    #[serde(default = "default_schema_version")]
    pub schema_version: u64,
}

impl EdgeSchema {
    /// Validate that the schema has compatible CSR strategies.
    /// At least one of out-edge or in-edge must be enabled (not None).
    pub fn validate(&self) -> crate::core::StorageResult<()> {
        if self.oe_strategy == EdgeStrategy::None && self.ie_strategy == EdgeStrategy::None {
            return Err(crate::core::StorageError::invalid_operation(
                format!("EdgeSchema '{}': both oe_strategy and ie_strategy are None. \
                         At least one direction must be enabled", self.label_name),
            ));
        }
        Ok(())
    }

    /// Validate schema at creation time
    /// Ensures property names are valid and edge types are well-formed
    pub fn validate_on_creation(&self) -> crate::core::StorageResult<()> {
        // Validate edge name
        if self.label_name.is_empty() {
            return Err(crate::core::StorageError::invalid_operation(
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
                return Err(crate::core::StorageError::invalid_operation(
                    format!("Duplicate property name in edge type '{}': '{}'",
                            self.label_name, prop.name),
                ));
            }

            // Validate property name format
            if prop.name.is_empty() {
                return Err(crate::core::StorageError::invalid_operation(
                    format!("Property name cannot be empty in edge type '{}'",
                            self.label_name),
                ));
            }

            Self::validate_identifier_internal(&prop.name)?;

            // Validate property data types are not Empty or Null
            Self::validate_property_type_internal(&prop.data_type, &prop.name)?;
        }

        Ok(())
    }

    /// Validate that an identifier (name) follows valid rules
    fn validate_identifier_internal(name: &str) -> crate::core::StorageResult<()> {
        let first_char = match name.chars().next() {
            Some(c) => c,
            None => return Err(crate::core::StorageError::invalid_operation(
                "Identifier cannot be empty".to_string(),
            )),
        };

        if !first_char.is_ascii_alphabetic() && first_char != '_' {
            return Err(crate::core::StorageError::invalid_operation(format!(
                "Identifier '{}' must start with ASCII letter or underscore, got '{}'",
                name, first_char
            )));
        }

        for (i, c) in name.chars().enumerate() {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(crate::core::StorageError::invalid_operation(format!(
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
        data_type: &crate::core::DataType,
        prop_name: &str,
    ) -> crate::core::StorageResult<()> {
        use crate::core::DataType;

        match data_type {
            DataType::Empty => {
                return Err(crate::core::StorageError::invalid_operation(format!(
                    "Property '{}' cannot have type Empty - properties must have valid types",
                    prop_name
                )));
            }
            DataType::Null => {
                return Err(crate::core::StorageError::invalid_operation(format!(
                    "Property '{}' cannot have type Null - use nullable=true instead",
                    prop_name
                )));
            }
            _ => Ok(()),
        }
    }

    /// Increment schema version when schema changes
    pub fn increment_version(&mut self) {
        self.schema_version += 1;
    }

    /// Validate that the loaded schema matches the expected version.
    /// Returns Ok(()) if valid, Err with description if there are issues.
    ///
    /// Note: This method must be called explicitly by callers to enforce version checking.
    pub fn validate_version(&self, expected_version: u64) -> Result<(), String> {
        if self.schema_version != expected_version {
            return Err(format!(
                "Edge schema version mismatch: expected {}, got {}",
                expected_version, self.schema_version
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nbr {
    pub neighbor: VertexId,
    pub edge_id: EdgeId,
    pub prop_offset: u32,
    pub create_ts: Timestamp,
    pub delete_ts: Timestamp,
}

impl Nbr {
    pub fn new(
        neighbor: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        create_ts: Timestamp,
    ) -> Self {
        Self {
            neighbor,
            edge_id,
            prop_offset,
            create_ts,
            delete_ts: u32::MAX,
        }
    }

    pub fn with_delete_ts(
        neighbor: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        create_ts: Timestamp,
        delete_ts: Timestamp,
    ) -> Self {
        Self {
            neighbor,
            edge_id,
            prop_offset,
            create_ts,
            delete_ts,
        }
    }

    pub fn is_valid_at(&self, ts: Timestamp) -> bool {
        self.create_ts <= ts && ts < self.delete_ts
    }
}

/// Compact neighbor structure without edge_id field
///
/// Used in frozen segments to save memory when EdgeId is stored separately.
/// Size: 44 bytes (32-bit neighbor + 32-bit prop_offset + 32-bit create_ts + 32-bit delete_ts)
/// vs Nbr: 52 bytes (includes 64-bit edge_id)
///
/// EdgeId is stored separately in segment-level storage (segment_edge_ids) and
/// recovered at query time using position-based mapping, allowing for 15% memory savings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NbrWithoutEdgeId {
    pub neighbor: VertexId,      // 32 bits (variable length encoding)
    pub prop_offset: u32,        // 4 bytes
    pub create_ts: Timestamp,    // 4 bytes
    pub delete_ts: Timestamp,    // 4 bytes
}

impl NbrWithoutEdgeId {
    pub fn new(
        neighbor: VertexId,
        prop_offset: u32,
        create_ts: Timestamp,
    ) -> Self {
        Self {
            neighbor,
            prop_offset,
            create_ts,
            delete_ts: u32::MAX,
        }
    }

    pub fn with_delete_ts(
        neighbor: VertexId,
        prop_offset: u32,
        create_ts: Timestamp,
        delete_ts: Timestamp,
    ) -> Self {
        Self {
            neighbor,
            prop_offset,
            create_ts,
            delete_ts,
        }
    }

    /// Convert from regular Nbr (discarding edge_id)
    pub fn from_nbr(nbr: &Nbr) -> Self {
        Self {
            neighbor: nbr.neighbor,
            prop_offset: nbr.prop_offset,
            create_ts: nbr.create_ts,
            delete_ts: nbr.delete_ts,
        }
    }

    /// Convert back to Nbr (requires recovered edge_id)
    pub fn to_nbr(&self, edge_id: EdgeId) -> Nbr {
        Nbr {
            neighbor: self.neighbor,
            edge_id,
            prop_offset: self.prop_offset,
            create_ts: self.create_ts,
            delete_ts: self.delete_ts,
        }
    }

    pub fn is_valid_at(&self, ts: Timestamp) -> bool {
        self.create_ts <= ts && ts < self.delete_ts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImmutableNbr {
    pub neighbor: VertexId,
    pub edge_id: EdgeId,
    pub prop_offset: u32,
    pub timestamp: Timestamp,
}

impl ImmutableNbr {
    pub fn new(neighbor: VertexId, edge_id: EdgeId, prop_offset: u32) -> Self {
        Self::with_timestamp(neighbor, edge_id, prop_offset, 0)
    }

    pub fn with_timestamp(
        neighbor: VertexId,
        edge_id: EdgeId,
        prop_offset: u32,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            neighbor,
            edge_id,
            prop_offset,
            timestamp,
        }
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
        assert!(result.unwrap_err().to_string().contains("both oe_strategy and ie_strategy are None"));
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

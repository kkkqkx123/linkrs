//! Edge schema types: record shape, direction/multiplicity enums and
//! the validated `EdgeSchema` carried by the catalog.

use super::record_form_policy::{validate_record_form_target, RecordForm};
use crate::types::StoragePropertyDef;
use graphdb_core::types::{EdgeStrategy, LabelId, VertexId};
use graphdb_core::{Edge, Value};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct EdgeRecord {
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub rank: i64,
    pub properties: Vec<(Arc<str>, Value)>,
}

impl From<&EdgeRecord> for Edge {
    fn from(record: &EdgeRecord) -> Self {
        let props: std::collections::HashMap<Arc<str>, Value> =
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

    /// Validate a resolved schema carrying its authoritative record form.
    ///
    /// Central entry for all strategy-plus-form rules so construction, load
    /// and migration report one conflict with one way out. Checks direction
    /// presence, single-plus-inline pairing, pure arity and bundled
    /// admission through the shared helpers.
    pub fn validate_resolved(&self) -> graphdb_core::StorageResult<()> {
        self.validate()?;
        validate_record_form_target(
            &self.properties,
            self.oe_strategy,
            self.ie_strategy,
            self.record_form,
        )
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

//! Schema change event tracking and history management
//!
//! This module provides the foundation for recording schema modifications,
//! enabling version history tracking and compatibility analysis.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::core::{DataType, Value};

/// Type identifier for change tracking - distinguishes between vertex and edge schema changes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaObjectType {
    Vertex,
    Edge,
}

/// Detailed information about schema modifications
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChangeDetails {
    /// Property added: (property_name, data_type, nullable, default_value)
    PropertyAdded {
        name: String,
        data_type: DataType,
        nullable: bool,
        default_value: Option<Value>,
    },

    /// Property removed: property_name
    PropertyRemoved {
        name: String,
        data_type: DataType,
    },

    /// Property renamed: (old_name, new_name)
    PropertyRenamed {
        old_name: String,
        new_name: String,
    },

    /// Property type modified: (property_name, old_type, new_type)
    PropertyTypeModified {
        name: String,
        old_type: DataType,
        new_type: DataType,
    },

    /// Property nullability changed
    PropertyNullabilityChanged {
        name: String,
        was_nullable: bool,
        now_nullable: bool,
    },

    /// Property default value changed
    PropertyDefaultValueChanged {
        name: String,
        old_default: Option<Value>,
        new_default: Option<Value>,
    },

    /// Primary key changed (for vertices)
    PrimaryKeyChanged {
        old_property: String,
        new_property: String,
    },
}

impl ChangeDetails {
    /// Check if this change is breaking (destructive)
    pub fn is_breaking(&self) -> bool {
        matches!(
            self,
            ChangeDetails::PropertyRemoved { .. }
                | ChangeDetails::PropertyTypeModified { .. }
                | ChangeDetails::PrimaryKeyChanged { .. }
        )
    }

    /// Get a human-readable description of the change
    pub fn description(&self) -> String {
        match self {
            ChangeDetails::PropertyAdded { name, .. } => {
                format!("Added property '{}'", name)
            }
            ChangeDetails::PropertyRemoved { name, .. } => {
                format!("Removed property '{}'", name)
            }
            ChangeDetails::PropertyRenamed {
                old_name,
                new_name,
            } => {
                format!("Renamed property '{}' to '{}'", old_name, new_name)
            }
            ChangeDetails::PropertyTypeModified {
                name,
                old_type,
                new_type,
            } => {
                format!(
                    "Modified property '{}' type from {:?} to {:?}",
                    name, old_type, new_type
                )
            }
            ChangeDetails::PropertyNullabilityChanged {
                name,
                was_nullable,
                now_nullable,
            } => {
                format!(
                    "Changed property '{}' nullability from {} to {}",
                    name, was_nullable, now_nullable
                )
            }
            ChangeDetails::PropertyDefaultValueChanged {
                name,
                old_default,
                new_default,
            } => {
                format!(
                    "Changed property '{}' default value from {:?} to {:?}",
                    name, old_default, new_default
                )
            }
            ChangeDetails::PrimaryKeyChanged {
                old_property,
                new_property,
            } => {
                format!(
                    "Changed primary key from '{}' to '{}'",
                    old_property, new_property
                )
            }
        }
    }
}

/// Property change event - represents a single property schema modification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyChange {
    /// Version at which this change occurred
    pub version: u64,
    /// Timestamp in milliseconds when the change was made
    pub timestamp_ms: u64,
    /// Type of object being modified
    pub object_type: SchemaObjectType,
    /// Label ID of the object
    pub label_id: u32,
    /// Label name of the object
    pub label_name: String,
    /// Detailed information about the change
    pub details: ChangeDetails,
}

impl PropertyChange {
    /// Create a new property change event
    pub fn new(
        version: u64,
        object_type: SchemaObjectType,
        label_id: u32,
        label_name: String,
        details: ChangeDetails,
    ) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        Self {
            version,
            timestamp_ms,
            object_type,
            label_id,
            label_name,
            details,
        }
    }

    /// Check if this change is breaking
    pub fn is_breaking(&self) -> bool {
        self.details.is_breaking()
    }

    /// Get change description
    pub fn description(&self) -> String {
        format!(
            "[v{}] {}: {}",
            self.version,
            self.label_name,
            self.details.description()
        )
    }
}

/// Batch of schema changes for a single label
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeLog {
    /// Object type (vertex or edge)
    pub object_type: SchemaObjectType,
    /// Label ID
    pub label_id: u32,
    /// Label name
    pub label_name: String,
    /// All changes for this label, indexed by version
    pub changes: HashMap<u64, Vec<PropertyChange>>,
}

impl ChangeLog {
    /// Create a new change log
    pub fn new(
        object_type: SchemaObjectType,
        label_id: u32,
        label_name: String,
    ) -> Self {
        Self {
            object_type,
            label_id,
            label_name,
            changes: HashMap::new(),
        }
    }

    /// Add a change to the log
    pub fn add_change(&mut self, change: PropertyChange) {
        self.changes
            .entry(change.version)
            .or_insert_with(Vec::new)
            .push(change);
    }

    /// Get changes for a specific version
    pub fn get_version_changes(&self, version: u64) -> Option<&Vec<PropertyChange>> {
        self.changes.get(&version)
    }

    /// Get all versions in order
    pub fn get_versions(&self) -> Vec<u64> {
        let mut versions: Vec<_> = self.changes.keys().copied().collect();
        versions.sort_unstable();
        versions
    }

    /// Get the latest version
    pub fn latest_version(&self) -> Option<u64> {
        self.changes.keys().max().copied()
    }

    /// Get all breaking changes
    pub fn get_breaking_changes(&self) -> Vec<PropertyChange> {
        self.changes
            .values()
            .flat_map(|changes| {
                changes
                    .iter()
                    .filter(|c| c.is_breaking())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_property_change_creation() {
        let change = PropertyChange::new(
            1,
            SchemaObjectType::Vertex,
            1,
            "User".to_string(),
            ChangeDetails::PropertyAdded {
                name: "email".to_string(),
                data_type: DataType::String,
                nullable: false,
                default_value: None,
            },
        );

        assert_eq!(change.version, 1);
        assert_eq!(change.object_type, SchemaObjectType::Vertex);
        assert_eq!(change.label_name, "User");
        assert!(!change.is_breaking());
    }

    #[test]
    fn test_breaking_changes() {
        let breaking_change = PropertyChange::new(
            1,
            SchemaObjectType::Vertex,
            1,
            "User".to_string(),
            ChangeDetails::PropertyRemoved {
                name: "old_field".to_string(),
                data_type: DataType::String,
            },
        );

        assert!(breaking_change.is_breaking());
    }

    #[test]
    fn test_changelog_operations() {
        let mut log = ChangeLog::new(SchemaObjectType::Vertex, 1, "User".to_string());

        let change1 = PropertyChange::new(
            1,
            SchemaObjectType::Vertex,
            1,
            "User".to_string(),
            ChangeDetails::PropertyAdded {
                name: "email".to_string(),
                data_type: DataType::String,
                nullable: false,
                default_value: None,
            },
        );

        log.add_change(change1);

        assert_eq!(log.get_versions(), vec![1]);
        assert_eq!(log.latest_version(), Some(1));
        assert!(log.get_version_changes(1).is_some());
    }
}

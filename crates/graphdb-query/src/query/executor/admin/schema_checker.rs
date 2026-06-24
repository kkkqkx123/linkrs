//! Schema compatibility checker
//!
//! Provides DDL layer compatibility checking for schema modifications,
//! detecting breaking changes and incompatibilities before applying them.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::core::StorageResult;
use crate::storage::{StorageReader, ChangeDetails};
use crate::core::DataType;

/// Schema compatibility check report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompatibilityReport {
    /// Whether there are breaking changes
    pub has_breaking_changes: bool,

    /// List of breaking changes (if any)
    pub breaking_changes: Vec<String>,

    /// List of warning messages
    pub warnings: Vec<String>,

    /// Estimated number of affected rows
    pub affected_data_count: u64,

    /// Recommended action
    pub recommendation: String,
}

impl CompatibilityReport {
    /// Check if schema modification is safe
    pub fn is_safe(&self) -> bool {
        self.breaking_changes.is_empty() && self.warnings.is_empty()
    }

    /// Print a summary of the report
    pub fn print_summary(&self) {
        if self.is_safe() {
            println!("✅ Compatible - No issues detected");
        } else {
            if !self.breaking_changes.is_empty() {
                println!("⚠️  BREAKING CHANGES:");
                for change in &self.breaking_changes {
                    println!("   - {}", change);
                }
            }
            if !self.warnings.is_empty() {
                println!("⚠️  WARNINGS:");
                for warn in &self.warnings {
                    println!("   - {}", warn);
                }
            }
        }
    }
}

/// Schema compatibility checker
pub struct SchemaCompatibilityChecker {
    storage_reader: Arc<dyn StorageReader>,
}

impl SchemaCompatibilityChecker {
    /// Create a new schema compatibility checker
    pub fn new(storage_reader: Arc<dyn StorageReader>) -> Self {
        Self { storage_reader }
    }

    /// Check compatibility before altering a vertex tag
    pub fn check_alter_vertex_compatibility(
        &self,
        space: &str,
        tag: &str,
        changes: &[crate::storage::PropertyChange],
    ) -> StorageResult<CompatibilityReport> {
        Self::vertex_compatibility(&*self.storage_reader, space, tag, changes)
    }

    /// Check compatibility before altering an edge type
    pub fn check_alter_edge_compatibility(
        &self,
        space: &str,
        edge_type: &str,
        changes: &[crate::storage::PropertyChange],
    ) -> StorageResult<CompatibilityReport> {
        Self::edge_compatibility(&*self.storage_reader, space, edge_type, changes)
    }

    /// Check vertex tag changes using a provided reader (static entry point)
    pub fn vertex_compatibility(
        reader: &dyn StorageReader,
        space: &str,
        tag: &str,
        changes: &[crate::storage::PropertyChange],
    ) -> StorageResult<CompatibilityReport> {
        let mut report = CompatibilityReport {
            has_breaking_changes: false,
            breaking_changes: Vec::new(),
            warnings: Vec::new(),
            affected_data_count: 0,
            recommendation: String::new(),
        };

        // Get current version history
        let current_history = reader.get_vertex_version_history(space, tag)?;

        let current_version = current_history
            .as_ref()
            .map(|h| h.latest_version())
            .unwrap_or(1);

        // Analyze each change
        for change in changes {
            Self::analyze_single_change(change, &mut report, current_version);
        }

        // Estimate affected data
        report.affected_data_count = reader.scan_vertices_by_tag(space, tag).map(|v| v.len() as u64).unwrap_or(0);

        // Generate recommendation
        if report.has_breaking_changes {
            report.recommendation = format!(
                "Data migration required for ~{} rows. Consider using backup before proceeding.",
                report.affected_data_count
            );
        } else if !report.warnings.is_empty() {
            report.recommendation = "Review warnings before proceeding".to_string();
        } else {
            report.recommendation = "Safe to apply".to_string();
        }

        Ok(report)
    }

    /// Check edge type changes using a provided reader (static entry point)
    pub fn edge_compatibility(
        reader: &dyn StorageReader,
        space: &str,
        edge_type: &str,
        changes: &[crate::storage::PropertyChange],
    ) -> StorageResult<CompatibilityReport> {
        let mut report = CompatibilityReport {
            has_breaking_changes: false,
            breaking_changes: Vec::new(),
            warnings: Vec::new(),
            affected_data_count: 0,
            recommendation: String::new(),
        };

        // Get current version history
        let current_history = reader.get_edge_version_history(space, edge_type)?;

        let current_version = current_history
            .as_ref()
            .map(|h| h.latest_version())
            .unwrap_or(1);

        // Analyze each change
        for change in changes {
            Self::analyze_single_change(change, &mut report, current_version);
        }

        // Estimate affected data
        report.affected_data_count = reader.scan_edges_by_type(space, edge_type).map(|e| e.len() as u64).unwrap_or(0);

        // Generate recommendation
        if report.has_breaking_changes {
            report.recommendation = format!(
                "Data migration required for ~{} edges. Consider using backup before proceeding.",
                report.affected_data_count
            );
        } else if !report.warnings.is_empty() {
            report.recommendation = "Review warnings before proceeding".to_string();
        } else {
            report.recommendation = "Safe to apply".to_string();
        }

        Ok(report)
    }

    /// Analyze a single schema change
    fn analyze_single_change(
        change: &crate::storage::PropertyChange,
        report: &mut CompatibilityReport,
        _current_version: u64,
    ) {
        match &change.details {
            ChangeDetails::PropertyRemoved { name, data_type: _ } => {
                report.has_breaking_changes = true;
                report.breaking_changes.push(format!(
                    "Removing property '{}' - existing data will be lost",
                    name
                ));
            }

            ChangeDetails::PropertyAdded {
                name,
                nullable,
                default_value,
                ..
            } => {
                if !nullable && default_value.is_none() {
                    report.warnings.push(format!(
                        "Adding required (not-null) property '{}' without default value. Existing rows cannot satisfy the not-null constraint",
                        name
                    ));
                }
            }

            ChangeDetails::PropertyRenamed { old_name, new_name } => {
                report.warnings.push(format!(
                    "Renaming property '{}' to '{}' - update queries accordingly",
                    old_name, new_name
                ));
            }

            ChangeDetails::PropertyTypeModified {
                name,
                old_type,
                new_type,
            } => {
                // Check type compatibility
                if !Self::are_types_compatible(old_type, new_type) {
                    report.has_breaking_changes = true;
                    report.breaking_changes.push(format!(
                        "Type change for '{}': {:?} → {:?} (incompatible)",
                        name, old_type, new_type
                    ));
                } else {
                    report.warnings.push(format!(
                        "Type change for '{}': {:?} → {:?} (compatible but verify)",
                        name, old_type, new_type
                    ));
                }
            }

            ChangeDetails::PropertyNullabilityChanged {
                name,
                was_nullable,
                now_nullable,
            } => {
                if *was_nullable && !now_nullable {
                    report.has_breaking_changes = true;
                    report.breaking_changes.push(format!(
                        "Property '{}' changed from nullable to not-nullable - may have NULL values",
                        name
                    ));
                } else {
                    report.warnings.push(format!(
                        "Property '{}' nullability changed from {} to {}",
                        name, was_nullable, now_nullable
                    ));
                }
            }

            ChangeDetails::PropertyDefaultValueChanged { name, old_default, new_default } => {
                if old_default != new_default {
                    report.warnings.push(format!(
                        "Property '{}' default value changed from {:?} to {:?}",
                        name, old_default, new_default
                    ));
                }
            }

            ChangeDetails::PrimaryKeyChanged {
                old_property,
                new_property,
            } => {
                report.has_breaking_changes = true;
                report.breaking_changes.push(format!(
                    "Primary key changed from '{}' to '{}' - may affect uniqueness constraints",
                    old_property, new_property
                ));
            }
        }
    }

    /// Check if two data types are compatible
    fn are_types_compatible(from: &DataType, to: &DataType) -> bool {
        match (from, to) {
            // Same type is always compatible
            (a, b) if std::mem::discriminant(a) == std::mem::discriminant(b) => true,
            // Other conversions are not compatible
            _ => false,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MockStorage;

    #[test]
    fn test_property_removal_is_breaking() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let checker = SchemaCompatibilityChecker::new(Arc::new(mock));
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyRemoved {
                name: "email".to_string(),
                data_type: DataType::String,
            },
        };

        let report = checker.check_alter_vertex_compatibility(
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(report.has_breaking_changes);
        assert_eq!(report.breaking_changes.len(), 1);
        assert!(report.breaking_changes[0].contains("Removing property"));
    }

    #[test]
    fn test_nullable_property_addition_is_safe() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let checker = SchemaCompatibilityChecker::new(Arc::new(mock));
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyAdded {
                name: "nickname".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
        };

        let report = checker.check_alter_vertex_compatibility(
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(!report.has_breaking_changes);
    }

    #[test]
    fn test_non_nullable_property_without_default_warns() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let checker = SchemaCompatibilityChecker::new(Arc::new(mock));
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyAdded {
                name: "status".to_string(),
                data_type: DataType::String,
                nullable: false,
                default_value: None,
            },
        };

        let report = checker.check_alter_vertex_compatibility(
            "test_space",
            "User",
            &[change],
        ).unwrap();

        // This is a warning, not breaking - the operation will fail at runtime for existing rows
        assert!(!report.has_breaking_changes);
        assert!(!report.warnings.is_empty());
        assert!(report.warnings[0].contains("not-null constraint"));
    }

    #[test]
    fn test_primary_key_change_is_breaking() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let checker = SchemaCompatibilityChecker::new(Arc::new(mock));
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PrimaryKeyChanged {
                old_property: "id".to_string(),
                new_property: "new_id".to_string(),
            },
        };

        let report = checker.check_alter_vertex_compatibility(
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(report.has_breaking_changes);
    }

    #[test]
    fn test_type_change_same_type_compatible() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let checker = SchemaCompatibilityChecker::new(Arc::new(mock));
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyTypeModified {
                name: "age".to_string(),
                old_type: DataType::BigInt,
                new_type: DataType::BigInt,
            },
        };

        let report = checker.check_alter_vertex_compatibility(
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(!report.has_breaking_changes);
    }

    #[test]
    fn test_vertex_compatibility_static_method() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyRemoved {
                name: "email".to_string(),
                data_type: DataType::String,
            },
        };

        let report = SchemaCompatibilityChecker::vertex_compatibility(
            &mock,
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(report.has_breaking_changes);
        assert_eq!(report.breaking_changes.len(), 1);
    }

    #[test]
    fn test_edge_compatibility_static_method() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyRemoved {
                name: "weight".to_string(),
                data_type: DataType::Double,
            },
        };

        let report = SchemaCompatibilityChecker::edge_compatibility(
            &mock,
            "test_space",
            "Knows",
            &[change],
        ).unwrap();

        assert!(report.has_breaking_changes);
        assert_eq!(report.breaking_changes.len(), 1);
    }

    #[test]
    fn test_vertex_compatibility_property_rename() {
        let mock = MockStorage::new().expect("Failed to create MockStorage");
        let change = crate::storage::PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyRenamed {
                old_name: "old_field".to_string(),
                new_name: "new_field".to_string(),
            },
        };

        let report = SchemaCompatibilityChecker::vertex_compatibility(
            &mock,
            "test_space",
            "User",
            &[change],
        ).unwrap();

        assert!(!report.has_breaking_changes);
        assert!(!report.warnings.is_empty());
        assert!(report.warnings[0].contains("Renaming property"));
    }
}

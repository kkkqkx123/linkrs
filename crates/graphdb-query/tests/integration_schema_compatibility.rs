//! Integration tests for schema compatibility checking
//!
//! Tests the SchemaCompatibilityChecker integrated with DDL operations

#[cfg(test)]
mod schema_compatibility_tests {
    use graphdb_query::query::executor::admin::CompatibilityReport;
    use graphdb_query::storage::{PropertyChange, ChangeDetails};
    use graphdb_query::core::DataType;

    #[test]
    fn test_compatibility_report_creation() {
        let report = CompatibilityReport {
            has_breaking_changes: false,
            breaking_changes: vec![],
            warnings: vec![],
            affected_data_count: 0,
            recommendation: "Safe to apply".to_string(),
        };

        assert!(report.is_safe());
        assert_eq!(report.recommendation, "Safe to apply");
    }

    #[test]
    fn test_compatibility_report_with_breaking_changes() {
        let report = CompatibilityReport {
            has_breaking_changes: true,
            breaking_changes: vec!["Removing property 'email'".to_string()],
            warnings: vec![],
            affected_data_count: 100,
            recommendation: "Data migration required for ~100 rows".to_string(),
        };

        assert!(!report.is_safe());
        assert!(report.has_breaking_changes);
        assert_eq!(report.breaking_changes.len(), 1);
    }

    #[test]
    fn test_compatibility_report_with_warnings_only() {
        let report = CompatibilityReport {
            has_breaking_changes: false,
            breaking_changes: vec![],
            warnings: vec!["Property renamed from 'old_name' to 'new_name'".to_string()],
            affected_data_count: 0,
            recommendation: "Review warnings before proceeding".to_string(),
        };

        assert!(!report.is_safe());
        assert!(!report.has_breaking_changes);
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn test_property_removal_detected() {
        let change = PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyRemoved {
                name: "email".to_string(),
                data_type: DataType::String,
            },
        };

        assert!(change.details.is_breaking());
    }

    #[test]
    fn test_property_addition_not_breaking() {
        let change = PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyAdded {
                name: "nickname".to_string(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
        };

        assert!(!change.details.is_breaking());
    }

    #[test]
    fn test_primary_key_change_is_breaking() {
        let change = PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PrimaryKeyChanged {
                old_property: "id".to_string(),
                new_property: "uuid".to_string(),
            },
        };

        assert!(change.details.is_breaking());
    }

    #[test]
    fn test_property_type_modification_is_breaking() {
        let change = PropertyChange {
            version: 1,
            timestamp_ms: 0,
            details: ChangeDetails::PropertyTypeModified {
                name: "age".to_string(),
                old_type: DataType::String,
                new_type: DataType::Int,
            },
        };

        assert!(change.details.is_breaking());
    }
}

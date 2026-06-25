use graphdb_core::core::{StorageError, StorageResult};
use graphdb_storage::storage::{
    ChangeDetails, PropertyChange, StorageReader,
};

use crate::converter::ConversionError;
use crate::plan::{MigrationPlan, MigrationStep, MigrationTarget, SafetyLevel, VersionRange};

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("Storage error: {0}")]
    Storage(Box<StorageError>),

    #[error("Plan error: {0}")]
    Plan(String),

    #[error("Conversion error: {0}")]
    Conversion(#[from] ConversionError),
}

impl From<StorageError> for MigrationError {
    fn from(e: StorageError) -> Self {
        MigrationError::Storage(Box::new(e))
    }
}

pub fn generate_vertex_plan(
    reader: &dyn StorageReader,
    space: &str,
    tag: &str,
    from_version: u64,
    to_version: u64,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader
        .get_vertex_schema_changes(space, tag, from_version, to_version)?;

    let steps: Vec<MigrationStep> =
        changes.iter().flat_map(step_from_change).collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_vertex_rows(reader, space, tag).unwrap_or(0);

     let target = MigrationTarget {
        space: space.to_string(),
        label: tag.to_string(),
        is_edge: false,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> =
            steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };

    Ok(MigrationPlan::new(
        target,
        version_range,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    ))
}

pub fn generate_edge_plan(
    reader: &dyn StorageReader,
    space: &str,
    edge_type: &str,
    from_version: u64,
    to_version: u64,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader
        .get_edge_schema_changes(space, edge_type, from_version, to_version)?;

    let steps: Vec<MigrationStep> =
        changes.iter().flat_map(step_from_change).collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_edge_rows(reader, space, edge_type).unwrap_or(0);

    let target = MigrationTarget {
        space: space.to_string(),
        label: edge_type.to_string(),
        is_edge: true,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> =
            steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };

    Ok(MigrationPlan::new(
        target,
        version_range,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    ))
}

fn estimate_vertex_rows(reader: &dyn StorageReader, space: &str, tag: &str) -> StorageResult<u64> {
    reader.count_vertices_by_tag(space, tag)
}

fn estimate_edge_rows(reader: &dyn StorageReader, space: &str, edge_type: &str) -> StorageResult<u64> {
    reader.count_edges_by_type(space, edge_type)
}

fn step_from_change(change: &PropertyChange) -> Vec<MigrationStep> {
    match &change.details {
        ChangeDetails::PropertyAdded { name, data_type, nullable, default_value } => {
            vec![MigrationStep::AddColumn {
                name: name.clone(),
                data_type: data_type.clone(),
                nullable: *nullable,
                default_value: default_value.clone(),
            }]
        }
        ChangeDetails::PropertyRemoved { name, data_type: _ } => {
            vec![MigrationStep::DropColumn {
                name: name.clone(),
            }]
        }
        ChangeDetails::PropertyRenamed { old_name, new_name } => {
            vec![MigrationStep::RenameColumn {
                old_name: old_name.clone(),
                new_name: new_name.clone(),
            }]
        }
        ChangeDetails::PropertyTypeModified { name, old_type, new_type } => {
            vec![MigrationStep::ConvertType {
                name: name.clone(),
                from_type: old_type.clone(),
                to_type: new_type.clone(),
            }]
        }
        ChangeDetails::PropertyNullabilityChanged { name, was_nullable, now_nullable } => {
            vec![MigrationStep::ChangeNullability {
                name: name.clone(),
                was_nullable: *was_nullable,
                now_nullable: *now_nullable,
            }]
        }
        ChangeDetails::PropertyDefaultValueChanged { name, old_default: _, new_default } => {
            vec![MigrationStep::SetDefault {
                name: name.clone(),
                default_value: new_default.clone(),
            }]
        }
        ChangeDetails::PrimaryKeyChanged { .. } => {
            vec![]
        }
    }
}

fn calculate_safety(steps: &[MigrationStep]) -> SafetyLevel {
    let mut has_dangerous = false;
    for step in steps {
        match step.safety_level() {
            SafetyLevel::Dangerous => has_dangerous = true,
            SafetyLevel::Warning => {}
            SafetyLevel::Safe => {}
        }
    }
    if has_dangerous {
        SafetyLevel::Dangerous
    } else if steps.iter().any(|s| s.safety_level() == SafetyLevel::Warning) {
        SafetyLevel::Warning
    } else {
        SafetyLevel::Safe
    }
}

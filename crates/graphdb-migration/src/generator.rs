use graphdb_core::core::StorageResult;
use graphdb_storage::storage::{
    ChangeDetails, PropertyChange, StorageReader,
};

use crate::converter::ConversionError;
use crate::plan::{MigrationPlan, MigrationStep, SafetyLevel};

#[derive(Debug, Clone)]
pub enum MigrationError {
    Storage(String),
    Plan(String),
    Conversion(String),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::Storage(msg) => write!(f, "Storage error: {}", msg),
            MigrationError::Plan(msg) => write!(f, "Plan error: {}", msg),
            MigrationError::Conversion(msg) => write!(f, "Conversion error: {}", msg),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<graphdb_core::core::StorageError> for MigrationError {
    fn from(e: graphdb_core::core::StorageError) -> Self {
        MigrationError::Storage(e.to_string())
    }
}

impl From<ConversionError> for MigrationError {
    fn from(e: ConversionError) -> Self {
        MigrationError::Conversion(e.message)
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
        changes.iter().flat_map(|c| step_from_change(c)).collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_vertex_rows(reader, space, tag).unwrap_or(0);

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> =
            steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan {
                space: space.to_string(),
                label: tag.to_string(),
                is_edge: false,
                from_version,
                to_version,
                steps: rollback_steps,
                estimated_rows,
                overall_safety: safety,
                rollback_plan: None,
            }))
        }
    } else {
        None
    };

    Ok(MigrationPlan {
        space: space.to_string(),
        label: tag.to_string(),
        is_edge: false,
        from_version,
        to_version,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    })
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
        changes.iter().flat_map(|c| step_from_change(c)).collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = 0;

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> =
            steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan {
                space: space.to_string(),
                label: edge_type.to_string(),
                is_edge: true,
                from_version,
                to_version,
                steps: rollback_steps,
                estimated_rows,
                overall_safety: safety,
                rollback_plan: None,
            }))
        }
    } else {
        None
    };

    Ok(MigrationPlan {
        space: space.to_string(),
        label: edge_type.to_string(),
        is_edge: true,
        from_version,
        to_version,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    })
}

fn estimate_vertex_rows(reader: &dyn StorageReader, space: &str, tag: &str) -> StorageResult<u64> {
    let vertices = reader.scan_vertices_by_tag(space, tag)?;
    Ok(vertices.len() as u64)
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

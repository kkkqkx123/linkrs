use std::collections::HashMap;

use graphdb_core::core::{Tag, Value, Vertex};
use graphdb_storage::storage::StorageClient;

use crate::converter::convert_value;
use crate::generator::MigrationError;
use crate::plan::{MigrationPlan, MigrationReport, MigrationStep, SafetyLevel};

pub fn execute_migration_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError> {
    if plan.is_empty() {
        return Ok(MigrationReport {
            success: true,
            steps_completed: 0,
            rows_migrated: 0,
            errors: vec![],
        });
    }

    if plan.is_edge {
        execute_edge_plan(storage, plan)
    } else {
        execute_vertex_plan(storage, plan)
    }
}

fn execute_vertex_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError> {
    let mut rows_migrated = 0u64;
    let mut errors = Vec::new();

    for (step_idx, step) in plan.steps.iter().enumerate() {
        if !step.is_data_modifying() {
            continue;
        }

        let vertices = storage
            .scan_vertices_by_tag(&plan.space, &plan.label)
            .map_err(|e| MigrationError::Storage(e.to_string()))?;

        let mut migrated = 0u64;
        for vertex in &vertices {
            match apply_step_to_vertex(vertex, &plan.label, step) {
                Ok(Some(transformed)) => {
                    storage
                        .update_vertex(&plan.space, transformed)
                        .map_err(|e| MigrationError::Storage(e.to_string()))?;
                    migrated += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    errors.push(format!(
                        "Step {} ({}) vertex {}: {}",
                        step_idx + 1,
                        step.description(),
                        vertex.vid,
                        e
                    ));
                }
            }
        }

        rows_migrated += migrated;
    }

    let success = errors.is_empty();
    Ok(MigrationReport {
        success,
        steps_completed: plan.steps.len(),
        rows_migrated,
        errors,
    })
}

fn execute_edge_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError> {
    let edges = storage
        .scan_edges_by_type(&plan.space, &plan.label)
        .map_err(|e| MigrationError::Storage(e.to_string()))?;

    let mut rows_migrated = 0u64;
    let mut errors = Vec::new();

    for (step_idx, step) in plan.steps.iter().enumerate() {
        if !step.is_data_modifying() {
            continue;
        }

        for edge in &edges {
            match apply_step_to_edge(edge, step) {
                Ok(new_props) => {
                    let mut transformed = edge.clone();
                    transformed.props = new_props;

                    storage
                        .delete_edge(&plan.space, &edge.src, &edge.dst, &edge.edge_type, edge.ranking)
                        .map_err(|e| MigrationError::Storage(e.to_string()))?;

                    storage
                        .insert_edge(&plan.space, transformed)
                        .map_err(|e| MigrationError::Storage(e.to_string()))?;

                    rows_migrated += 1;
                }
                Err(e) => {
                    errors.push(format!(
                        "Step {} ({}) edge ({:?}→{:?}): {}",
                        step_idx + 1,
                        step.description(),
                        edge.src,
                        edge.dst,
                        e
                    ));
                }
            }
        }
    }

    let success = errors.is_empty();
    Ok(MigrationReport {
        success,
        steps_completed: plan.steps.len(),
        rows_migrated,
        errors,
    })
}

pub fn rollback_migration(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError> {
    match &plan.rollback_plan {
        Some(rollback) => execute_migration_plan(storage, rollback),
        None => {
            if plan.overall_safety == SafetyLevel::Dangerous {
                Err(MigrationError::Plan(
                    "Cannot rollback a dangerous migration (data loss)".to_string(),
                ))
            } else {
                Err(MigrationError::Plan("No rollback plan available".to_string()))
            }
        }
    }
}

fn apply_step_to_vertex(
    vertex: &Vertex,
    label: &str,
    step: &MigrationStep,
) -> Result<Option<Vertex>, String> {
    let tag = match vertex.tags.iter().find(|t| t.name == label) {
        Some(t) => t,
        None => return Ok(None),
    };

    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match tag.properties.get(old_name) {
                Some(v) => v,
                None => return Ok(None),
            };
            let mut new_props = tag.properties.clone();
            new_props.remove(old_name);
            new_props.insert(new_name.clone(), value.clone());

            let new_tags: Vec<Tag> = vertex
                .tags
                .iter()
                .map(|t| {
                    if t.name == *label {
                        Tag::new(t.name.clone(), new_props.clone())
                    } else {
                        t.clone()
                    }
                })
                .collect();

            let mut v = vertex.clone();
            v.tags = new_tags;
            v.properties = merge_vertex_properties(&v.tags);
            Ok(Some(v))
        }
        MigrationStep::ConvertType { name, from_type: _, to_type } => {
            let value = match tag.properties.get(name) {
                Some(v) => v,
                None => return Ok(None),
            };
            let converted = convert_value(value, to_type).map_err(|e| e.message)?;

            let mut new_props = tag.properties.clone();
            new_props.insert(name.clone(), converted);

            let new_tags: Vec<Tag> = vertex
                .tags
                .iter()
                .map(|t| {
                    if t.name == *label {
                        Tag::new(t.name.clone(), new_props.clone())
                    } else {
                        t.clone()
                    }
                })
                .collect();

            let mut v = vertex.clone();
            v.tags = new_tags;
            v.properties = merge_vertex_properties(&v.tags);
            Ok(Some(v))
        }
        MigrationStep::DropColumn { name } => {
            if !tag.properties.contains_key(name) {
                return Ok(None);
            }
            let mut new_props = tag.properties.clone();
            new_props.remove(name);

            let new_tags: Vec<Tag> = vertex
                .tags
                .iter()
                .map(|t| {
                    if t.name == *label {
                        Tag::new(t.name.clone(), new_props.clone())
                    } else {
                        t.clone()
                    }
                })
                .collect();

            let mut v = vertex.clone();
            v.tags = new_tags;
            v.properties = merge_vertex_properties(&v.tags);
            Ok(Some(v))
        }
        MigrationStep::SetDefault { name, default_value } => {
            if tag.properties.contains_key(name) {
                return Ok(None);
            }
            let mut new_props = tag.properties.clone();
            new_props.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::core::value::null::NullType::Null)),
            );

            let new_tags: Vec<Tag> = vertex
                .tags
                .iter()
                .map(|t| {
                    if t.name == *label {
                        Tag::new(t.name.clone(), new_props.clone())
                    } else {
                        t.clone()
                    }
                })
                .collect();

            let mut v = vertex.clone();
            v.tags = new_tags;
            v.properties = merge_vertex_properties(&v.tags);
            Ok(Some(v))
        }
        MigrationStep::ChangeNullability { .. } => {
            if !tag.properties.contains_key(name_from_step(step)) {
                return Ok(None);
            }
            Ok(None)
        }
        MigrationStep::AddColumn { .. } => Ok(None),
    }
}

fn apply_step_to_edge(
    edge: &graphdb_core::core::Edge,
    step: &MigrationStep,
) -> Result<HashMap<String, Value>, String> {
    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match edge.props.get(old_name) {
                Some(v) => v.clone(),
                None => return Err(format!("Property '{}' not found on edge", old_name)),
            };
            let mut props = edge.props.clone();
            props.remove(old_name);
            props.insert(new_name.clone(), value);
            Ok(props)
        }
        MigrationStep::ConvertType { name, from_type: _, to_type } => {
            let value = match edge.props.get(name) {
                Some(v) => v,
                None => return Err(format!("Property '{}' not found on edge", name)),
            };
            let converted = convert_value(value, to_type).map_err(|e| e.message)?;
            let mut props = edge.props.clone();
            props.insert(name.clone(), converted);
            Ok(props)
        }
        MigrationStep::DropColumn { name } => {
            let mut props = edge.props.clone();
            props.remove(name);
            Ok(props)
        }
        MigrationStep::SetDefault { name, default_value } => {
            if edge.props.contains_key(name) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::core::value::null::NullType::Null)),
            );
            Ok(props)
        }
        MigrationStep::ChangeNullability { .. } => Ok(edge.props.clone()),
        MigrationStep::AddColumn { .. } => Ok(edge.props.clone()),
    }
}

fn name_from_step(step: &MigrationStep) -> &str {
    match step {
        MigrationStep::AddColumn { name, .. } => name,
        MigrationStep::DropColumn { name } => name,
        MigrationStep::RenameColumn { old_name, .. } => old_name,
        MigrationStep::ConvertType { name, .. } => name,
        MigrationStep::SetDefault { name, .. } => name,
        MigrationStep::ChangeNullability { name, .. } => name,
    }
}

fn merge_vertex_properties(tags: &[Tag]) -> HashMap<String, Value> {
    let mut merged = HashMap::new();
    for tag in tags {
        for (k, v) in &tag.properties {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged
}

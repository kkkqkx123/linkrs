use std::collections::HashMap;

use graphdb_core::{Edge, Tag, Value, Vertex};
use graphdb_storage::core::error::storage::StorageErrorKind;
use graphdb_storage::core::StorageError;
use graphdb_storage::{StorageClient, StorageWriter};

use crate::converter::convert_value;
use crate::generator::MigrationError;
use crate::plan::{MigrationPlan, MigrationReport, MigrationStep, SafetyLevel};

pub fn execute_migration_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError> {
    // Dropping a column permanently destroys the stored property values and
    // has no reverse step, so make sure the irreversibility is explicit
    // before anything is executed.
    for step in &plan.steps {
        if let MigrationStep::DropColumn { name } = step {
            log::warn!(
                "Migration plan contains an irreversible DropColumn step: column '{}' on \
                 {}/{} will be permanently removed and cannot be rolled back",
                name,
                plan.target.space,
                plan.target.label
            );
        }
    }

    let remaining = plan.remaining_steps();
    if remaining.is_empty() {
        return Ok(MigrationReport {
            success: true,
            steps_completed: plan.completed_steps.len(),
            rows_migrated: 0,
            errors: vec![],
            completed_step_indices: plan.completed_steps.clone(),
        });
    }

    if plan.target.is_edge {
        execute_edge_plan(storage, plan, &remaining)
    } else {
        execute_vertex_plan(storage, plan, &remaining)
    }
}

fn execute_vertex_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
    remaining: &[usize],
) -> Result<MigrationReport, MigrationError> {
    let vertices = storage.scan_vertices_by_tag(&plan.target.space, &plan.target.label)?;

    // Phase 1 (staging): apply every remaining data-modifying step to
    // in-memory copies of the scanned rows. No storage writes happen here,
    // so any transformation failure leaves the stored data untouched and
    // the migration is all-or-nothing instead of partially committed.
    let mut staged: Vec<Vertex> = Vec::new();
    let mut errors = Vec::new();
    'vertex_rows: for vertex in &vertices {
        let mut current = vertex.clone();
        for &step_idx in remaining {
            let step = &plan.steps[step_idx];
            if !step.is_data_modifying() {
                continue;
            }
            match apply_step_to_vertex(&current, &plan.target.label, step) {
                Ok(Some(next)) => current = next,
                Ok(None) => {}
                Err(e) => {
                    errors.push(format!(
                        "Step {} ({}) vertex {}: {}",
                        step_idx + 1,
                        step.description(),
                        vertex.vid,
                        e
                    ));
                    continue 'vertex_rows;
                }
            }
        }
        staged.push(current);
    }

    if !errors.is_empty() {
        return Ok(MigrationReport {
            success: false,
            steps_completed: plan.completed_steps.len(),
            rows_migrated: 0,
            errors,
            completed_step_indices: plan.completed_steps.clone(),
        });
    }

    // Phase 2 (commit): every row was fully transformed before the first
    // write. All writes run inside a single auto-commit group window so the
    // migration is atomic: one commit point at the end, and any storage-level
    // failure rolls back every already-written row through the shared undo log.
    let rows_migrated = staged.len() as u64;
    commit_staged_rows(storage, staged, |writer, vertex| {
        writer.update_vertex(&plan.target.space, vertex)
    })?;

    let completed_step_indices: Vec<usize> = plan
        .completed_steps
        .iter()
        .copied()
        .chain(remaining.iter().copied())
        .collect();
    Ok(MigrationReport {
        success: true,
        steps_completed: completed_step_indices.len(),
        rows_migrated,
        errors: vec![],
        completed_step_indices,
    })
}

fn execute_edge_plan(
    storage: &mut dyn StorageClient,
    plan: &MigrationPlan,
    remaining: &[usize],
) -> Result<MigrationReport, MigrationError> {
    let edges = storage.scan_edges_by_type(&plan.target.space, &plan.target.label)?;

    // Phase 1 (staging): transform all rows in memory first so a step or
    // conversion failure never leaves partially migrated data behind.
    let mut staged: Vec<Edge> = Vec::new();
    let mut errors = Vec::new();
    'edge_rows: for edge in &edges {
        let mut current = edge.clone();
        for &step_idx in remaining {
            let step = &plan.steps[step_idx];
            if !step.is_data_modifying() {
                continue;
            }
            match apply_step_to_edge(&current, step) {
                Ok(new_props) => current.props = new_props,
                Err(e) => {
                    errors.push(format!(
                        "Step {} ({}) edge ({:?}→{:?}): {}",
                        step_idx + 1,
                        step.description(),
                        edge.src,
                        edge.dst,
                        e
                    ));
                    continue 'edge_rows;
                }
            }
        }
        staged.push(current);
    }

    if !errors.is_empty() {
        return Ok(MigrationReport {
            success: false,
            steps_completed: plan.completed_steps.len(),
            rows_migrated: 0,
            errors,
            completed_step_indices: plan.completed_steps.clone(),
        });
    }

    // Phase 2 (commit): every row was fully transformed before the first
    // write. Same group-window commit as the vertex plan; see
    // `commit_staged_rows`.
    let rows_migrated = staged.len() as u64;
    commit_staged_rows(storage, staged, |writer, edge| {
        writer.update_edge(&plan.target.space, edge)
    })?;

    let completed_step_indices: Vec<usize> = plan
        .completed_steps
        .iter()
        .copied()
        .chain(remaining.iter().copied())
        .collect();
    Ok(MigrationReport {
        success: true,
        steps_completed: completed_step_indices.len(),
        rows_migrated,
        errors: vec![],
        completed_step_indices,
    })
}

/// Commit a batch of fully-staged rows atomically.
///
/// All rows are written through a single auto-commit group window: the
/// engine assigns one shared write timestamp and undo log, so either every
/// row becomes visible (one commit point in `finalize_auto_commit_group`)
/// or none does (`rollback_auto_commit_group` replays the shared undo log).
/// Storage backends without group support fall back to per-row auto-commit;
/// that path keeps the previous all-or-nothing guarantee for transformation
/// errors only, and a mid-loop failure may leave earlier rows committed.
fn commit_staged_rows<T>(
    storage: &mut dyn StorageClient,
    staged: Vec<T>,
    mut write_one: impl FnMut(&mut dyn StorageWriter, T) -> Result<(), StorageError>,
) -> Result<(), MigrationError> {
    let window = match storage.begin_auto_commit_group() {
        Ok(window) => Some(window),
        Err(e) if e.kind() == StorageErrorKind::NotSupported => {
            log::warn!(
                "Storage backend does not support auto-commit groups; \
                 falling back to per-row commits for migration"
            );
            None
        }
        Err(e) => return Err(MigrationError::Storage(Box::new(e))),
    };

    let Some(window) = window else {
        for row in staged {
            write_one(&mut *storage, row).map_err(MigrationError::from)?;
        }
        return Ok(());
    };

    let result = (|| {
        let mut writer = storage
            .bind_auto_commit_writer(&window)
            .map_err(MigrationError::from)?;
        for row in staged {
            write_one(&mut *writer, row).map_err(MigrationError::from)?;
        }
        Ok(())
    })();

    match result {
        Ok(()) => storage
            .finalize_auto_commit_group(&window)
            .map_err(MigrationError::from),
        Err(error) => {
            if let Err(rollback_error) = storage.rollback_auto_commit_group(&window) {
                log::error!("Migration rollback failed: {rollback_error}");
            }
            Err(error)
        }
    }
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
                Err(MigrationError::Plan(
                    "No rollback plan available".to_string(),
                ))
            }
        }
    }
}

fn apply_step_to_vertex(
    vertex: &Vertex,
    label: &str,
    step: &MigrationStep,
) -> Result<Option<Vertex>, String> {
    let mut v = vertex.clone();
    let tag = match v.tags.iter_mut().find(|t| t.name == label) {
        Some(t) => t,
        None => return Ok(None),
    };

    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match tag.properties.remove(old_name) {
                Some(v) => v,
                None => return Ok(None),
            };
            tag.properties.insert(new_name.clone(), value);
        }
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
            let value = match tag.properties.get(name) {
                Some(v) => v.clone(),
                None => return Ok(None),
            };
            let converted = convert_value(&value, to_type).map_err(|e| e.message)?;
            tag.properties.insert(name.clone(), converted);
        }
        MigrationStep::DropColumn { name } => {
            if !tag.properties.contains_key(name) {
                return Ok(None);
            }
            tag.properties.remove(name);
        }
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if tag.properties.contains_key(name) {
                return Ok(None);
            }
            tag.properties.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::ChangeNullability { .. } => return Ok(None),
        MigrationStep::AddColumn { .. } => return Ok(None),
    }

    v.properties = merge_vertex_properties(&v.tags);
    Ok(Some(v))
}

fn apply_step_to_edge(edge: &Edge, step: &MigrationStep) -> Result<HashMap<String, Value>, String> {
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
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
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
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if edge.props.contains_key(name) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
            Ok(props)
        }
        MigrationStep::ChangeNullability { .. } => Ok(edge.props.clone()),
        MigrationStep::AddColumn { .. } => Ok(edge.props.clone()),
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

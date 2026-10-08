//! Pre-migration backup for destructive steps.
//!
//! `DropColumn` has no inverse step, so a plan containing drops can only be
//! rolled back from a backup taken before execution. When the caller supplies
//! a backup directory, the executor snapshots the affected rows first and
//! rollback restores from that snapshot instead of failing outright.

use std::path::Path;

use linkrs_core::error::storage::StorageErrorKind;
use linkrs_core::{Edge, Vertex};
use linkrs_storage::{StorageReader, StorageSchemaOps, StorageWriter};
use serde::{Deserialize, Serialize};

use crate::error::MigrationError;
use crate::plan::{push_escaped_path_component, MigrationPlan, MigrationReport, MigrationStep};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DestructiveBackup {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub plan_hash: String,
    pub dropped_columns: Vec<String>,
    pub dropped_label: Option<String>,
    pub dropped_edge_type: Option<String>,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
}

fn backup_file_name(plan: &MigrationPlan) -> String {
    let hash = if plan.plan_hash.is_empty() {
        plan.compute_hash()
    } else {
        plan.plan_hash.clone()
    };
    let mut name = String::from("backup_");
    push_escaped_path_component(&mut name, &plan.target.space);
    name.push('-');
    push_escaped_path_component(&mut name, &plan.target.label);
    name.push('-');
    push_escaped_path_component(&mut name, &hash);
    name.push_str(".json");
    name
}

fn backup_path(plan: &MigrationPlan, dir: &Path) -> std::path::PathBuf {
    dir.join(backup_file_name(plan))
}

/// Snapshot rows touched by destructive steps. Returns the row count.
pub fn write_backup<S>(
    storage: &S,
    plan: &MigrationPlan,
    dir: &Path,
) -> Result<usize, MigrationError>
where
    S: StorageReader + ?Sized,
{
    std::fs::create_dir_all(dir)
        .map_err(|e| MigrationError::Checkpoint(format!("cannot create backup directory: {e}")))?;
    let mut backup = DestructiveBackup {
        space: plan.target.space.clone(),
        label: plan.target.label.clone(),
        is_edge: plan.target.is_edge,
        plan_hash: if plan.plan_hash.is_empty() {
            plan.compute_hash()
        } else {
            plan.plan_hash.clone()
        },
        dropped_columns: plan.dropped_columns(),
        dropped_label: None,
        dropped_edge_type: None,
        vertices: Vec::new(),
        edges: Vec::new(),
    };
    for step in &plan.steps {
        match step {
            MigrationStep::DropLabel { label_name } => {
                backup.dropped_label = Some(label_name.clone());
            }
            MigrationStep::DropEdgeType { edge_type_name } => {
                backup.dropped_edge_type = Some(edge_type_name.clone());
            }
            _ => {}
        }
    }
    if plan.target.is_edge {
        backup.edges = storage
            .scan_edges_by_type(&plan.target.space, &plan.target.label)
            .map_err(MigrationError::from)?;
    } else {
        backup.vertices = storage
            .scan_vertices_by_tag(&plan.target.space, &plan.target.label)
            .map_err(MigrationError::from)?;
    }
    let rows = backup.vertices.len() + backup.edges.len();
    let content = serde_json::to_string_pretty(&backup)
        .map_err(|e| MigrationError::Checkpoint(format!("serialize backup: {e}")))?;
    std::fs::write(backup_path(plan, dir), content)
        .map_err(|e| MigrationError::Checkpoint(format!("write backup: {e}")))?;
    Ok(rows)
}

fn load_backup(
    plan: &MigrationPlan,
    dir: &Path,
) -> Result<Option<DestructiveBackup>, MigrationError> {
    let path = backup_path(plan, dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| MigrationError::Checkpoint(format!("read backup: {e}")))?;
    let backup: DestructiveBackup = serde_json::from_str(&content)
        .map_err(|e| MigrationError::Checkpoint(format!("parse backup: {e}")))?;
    if !plan.plan_hash.is_empty()
        && !backup.plan_hash.is_empty()
        && backup.plan_hash != plan.plan_hash
    {
        return Err(MigrationError::Plan(format!(
            "backup hash mismatch for {}/{}: backup {} != plan {}",
            plan.target.space, plan.target.label, backup.plan_hash, plan.plan_hash
        )));
    }
    Ok(Some(backup))
}

/// Restore a destructive plan from its backup. Returns `None` when no backup
/// file exists for the plan.
pub fn restore_backup<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    dir: &Path,
) -> Result<Option<MigrationReport>, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + ?Sized,
{
    let backup = match load_backup(plan, dir)? {
        Some(b) => b,
        None => return Ok(None),
    };
    let mut restored: u64 = 0;
    if plan.target.is_edge {
        if backup.dropped_edge_type.is_some() {
            let info = linkrs_core::types::EdgeTypeInfo::new(plan.target.label.clone());
            match storage.create_edge_type(&plan.target.space, &info) {
                Ok(_) => {}
                Err(e) if e.kind() == StorageErrorKind::AlreadyExists => {}
                Err(e) => return Err(MigrationError::Storage(Box::new(e))),
            }
            for edge in &backup.edges {
                match storage.insert_edge(&plan.target.space, edge.clone()) {
                    Ok(_) => restored += 1,
                    Err(e) => log::warn!("Backup restore skipped one edge: {e}"),
                }
            }
        } else {
            let current = storage
                .scan_edges_by_type(&plan.target.space, &plan.target.label)
                .map_err(MigrationError::from)?;
            for saved in &backup.edges {
                let Some(mut live) = current
                    .iter()
                    .find(|e| {
                        e.src == saved.src && e.dst == saved.dst && e.ranking == saved.ranking
                    })
                    .cloned()
                else {
                    log::warn!(
                        "Backup restore skipped missing edge ({:?}->{:?})",
                        saved.src,
                        saved.dst
                    );
                    continue;
                };
                for col in &backup.dropped_columns {
                    if let Some(v) = saved.props.get(col.as_str()) {
                        live.props.insert(col.as_str().into(), v.clone());
                    }
                }
                storage
                    .update_edge(&plan.target.space, live)
                    .map_err(MigrationError::from)?;
                restored += 1;
            }
        }
    } else if backup.dropped_label.is_some() {
        let tag = linkrs_core::types::TagInfo::new(plan.target.label.clone());
        match storage.create_tag(&plan.target.space, &tag) {
            Ok(_) => {}
            Err(e) if e.kind() == StorageErrorKind::AlreadyExists => {}
            Err(e) => return Err(MigrationError::Storage(Box::new(e))),
        }
        for vertex in &backup.vertices {
            match storage.insert_vertex(&plan.target.space, vertex.clone()) {
                Ok(_) => restored += 1,
                Err(e) => log::warn!("Backup restore skipped vertex {}: {e}", vertex.vid),
            }
        }
    } else {
        let current = storage
            .scan_vertices_by_tag(&plan.target.space, &plan.target.label)
            .map_err(MigrationError::from)?;
        for saved in &backup.vertices {
            let Some(mut live) = current.iter().find(|v| v.vid == saved.vid).cloned() else {
                log::warn!("Backup restore skipped missing vertex {}", saved.vid);
                continue;
            };
            for col in &backup.dropped_columns {
                if let Some(v) = saved.tag.properties.get(col.as_str()) {
                    live.tag.properties.insert(col.as_str().into(), v.clone());
                }
            }
            storage
                .update_vertex(&plan.target.space, live)
                .map_err(MigrationError::from)?;
            restored += 1;
        }
    }
    let completed: Vec<usize> = (0..plan.steps.len()).collect();
    Ok(Some(MigrationReport {
        success: true,
        steps_completed: completed.len(),
        rows_migrated: restored,
        errors: vec![],
        completed_step_indices: completed,
    }))
}

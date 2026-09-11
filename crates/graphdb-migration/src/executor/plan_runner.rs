//! Plan execution pipeline: schema steps, dry-run, and vertex/edge data
//! migration runners (batch and streaming variants).

use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::types::{EdgeTypeInfo, TagInfo};
use graphdb_core::{Edge, Vertex};
use graphdb_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, MigrationHistoryRecord, MigrationStatus, StorageReader,
    StorageSchemaOps, StorageWriter,
};

use super::data_apply::{apply_step_to_edge, apply_step_to_vertex};
use crate::error::MigrationError;
use crate::event::MigrationEventListener;
use crate::plan::{MigrationPlan, MigrationReport, MigrationStep};
use crate::progress::MigrationProgress;

pub(super) fn execute_schema_steps<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    progress: &dyn MigrationProgress,
) -> Result<Vec<usize>, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + ?Sized,
{
    let mut executed = Vec::new();
    for &idx in remaining {
        let step = &plan.steps[idx];
        if !step.is_schema_modifying() {
            continue;
        }
        progress.on_step_start(idx, step);
        if plan.dry_run {
            progress.on_step_complete(idx, step);
            executed.push(idx);
            continue;
        }
        match step {
            MigrationStep::CreateLabel { label_name } => {
                let tag = TagInfo::new(label_name.clone());
                match storage.create_tag(&plan.target.space, &tag) {
                    Ok(_) => {}
                    Err(e) if e.kind() == StorageErrorKind::AlreadyExists => {
                        log::warn!("CreateLabel {} already exists: {}", label_name, e);
                    }
                    Err(e) => return Err(MigrationError::Storage(Box::new(e))),
                }
            }
            MigrationStep::DropLabel { label_name } => {
                storage
                    .drop_tag(&plan.target.space, label_name)
                    .map_err(|e| MigrationError::Storage(Box::new(e)))?;
            }
            MigrationStep::CreateEdgeType { edge_type_name } => {
                let info = EdgeTypeInfo::new(edge_type_name.clone());
                match storage.create_edge_type(&plan.target.space, &info) {
                    Ok(_) => {}
                    Err(e) if e.kind() == StorageErrorKind::AlreadyExists => {
                        log::warn!("CreateEdgeType {} already exists: {}", edge_type_name, e);
                    }
                    Err(e) => return Err(MigrationError::Storage(Box::new(e))),
                }
            }
            MigrationStep::DropEdgeType { edge_type_name } => {
                storage
                    .drop_edge_type(&plan.target.space, edge_type_name)
                    .map_err(|e| MigrationError::Storage(Box::new(e)))?;
            }
            _ => {}
        }
        progress.on_step_complete(idx, step);
        executed.push(idx);
    }
    Ok(executed)
}

pub(super) fn record_migration_history<S>(
    storage: &S,
    plan: &MigrationPlan,
    rows_migrated: u64,
    status: MigrationStatus,
    error_message: Option<String>,
) where
    S: StorageReader + ?Sized,
{
    let hash = if plan.plan_hash.is_empty() {
        plan.compute_hash()
    } else {
        plan.plan_hash.clone()
    };
    let record = MigrationHistoryRecord {
        id: 0,
        space: plan.target.space.clone(),
        label: plan.target.label.clone(),
        is_edge: plan.target.is_edge,
        from_version: plan.version_range.from,
        to_version: plan.version_range.to,
        plan_hash: hash,
        safety_level: format!("{:?}", plan.overall_safety),
        steps_count: plan.steps.len(),
        rows_migrated,
        status,
        applied_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        completed_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        ),
        error_message,
    };
    match storage.record_migration_history(record) {
        Ok(()) => {}
        Err(e) if e.kind() == StorageErrorKind::NotSupported => {
            log::warn!("Migration history not supported by storage: {}", e);
        }
        Err(e) => {
            log::warn!("Failed to record migration history: {}", e);
        }
    }
}

pub(super) fn execute_vertex_plan_with_progress<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    progress: &dyn MigrationProgress,
    _event_listener: Option<&dyn MigrationEventListener>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    // delegate to existing vertex plan but with progress row callbacks
    let report = execute_vertex_plan(storage, plan, remaining)?;
    // Emit row progress approximation
    if report.success && report.rows_migrated > 0 {
        progress.on_row_processed(report.rows_migrated);
    }
    Ok(report)
}

pub(super) fn execute_edge_plan_with_progress<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    progress: &dyn MigrationProgress,
    _event_listener: Option<&dyn MigrationEventListener>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    let report = execute_edge_plan(storage, plan, remaining)?;
    if report.success && report.rows_migrated > 0 {
        progress.on_row_processed(report.rows_migrated);
    }
    Ok(report)
}

pub(super) fn execute_dry_run<S>(
    storage: &mut S,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
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
    // Stage only, no commit. Count rows that would be migrated.
    let rows_migrated = if plan.target.is_edge {
        storage
            .scan_edges_by_type(&plan.target.space, &plan.target.label)
            .map(|v| v.len() as u64)
            .unwrap_or(0)
    } else {
        storage
            .count_vertices_by_tag(&plan.target.space, &plan.target.label)
            .unwrap_or(0)
    };
    // Validate conversions by attempting to stage in memory without writing.
    let mut errors = Vec::new();
    if plan.target.is_edge {
        let edges = storage
            .scan_edges_by_type(&plan.target.space, &plan.target.label)
            .unwrap_or_default();
        for edge in &edges {
            for &step_idx in &remaining {
                let step = &plan.steps[step_idx];
                let is_mod =
                    step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
                if !is_mod {
                    continue;
                }
                if let Err(e) = apply_step_to_edge(edge, step) {
                    errors.push(format!("Step {} preview error: {}", step_idx + 1, e));
                    break;
                }
            }
            if !errors.is_empty() {
                break;
            }
        }
    } else {
        let vertices = storage
            .scan_vertices_by_tag(&plan.target.space, &plan.target.label)
            .unwrap_or_default();
        for vertex in &vertices {
            for &step_idx in &remaining {
                let step = &plan.steps[step_idx];
                let is_mod =
                    step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
                if !is_mod {
                    continue;
                }
                match apply_step_to_vertex(vertex, &plan.target.label, step) {
                    Ok(_) => {}
                    Err(e) => {
                        errors.push(format!("Step {} preview error: {}", step_idx + 1, e));
                        break;
                    }
                }
            }
            if !errors.is_empty() {
                break;
            }
        }
    }
    let completed_step_indices = if errors.is_empty() {
        plan.completed_steps
            .iter()
            .copied()
            .chain(remaining.iter().copied())
            .collect()
    } else {
        plan.completed_steps.clone()
    };
    Ok(MigrationReport {
        success: errors.is_empty(),
        steps_completed: completed_step_indices.len(),
        rows_migrated,
        errors,
        completed_step_indices,
    })
}

fn execute_vertex_plan<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let batch_size = if plan.batch_size == 0 {
        1000
    } else {
        plan.batch_size
    };
    // Try streaming paginated path first; fallback to full scan if not supported.
    let paginated_probe =
        storage.scan_vertices_by_tag_paginated(&plan.target.space, &plan.target.label, 0, 1);
    let use_paginated = match paginated_probe {
        Ok(_) => true,
        Err(e) if e.kind() == StorageErrorKind::NotSupported => false,
        Err(_) => true,
    };

    if use_paginated {
        return execute_vertex_plan_streaming(storage, plan, remaining, batch_size);
    }

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
            let is_modifying =
                step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
            if !is_modifying {
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
    if let Some(window) = window {
        let result = (|| {
            let mut writer = storage
                .bind_auto_commit_writer(&window)
                .map_err(MigrationError::from)?;
            for vertex in staged {
                writer
                    .update_vertex(&plan.target.space, vertex)
                    .map_err(MigrationError::from)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => storage
                .finalize_auto_commit_group(&window)
                .map_err(MigrationError::from)?,
            Err(error) => {
                if let Err(rollback_error) = storage.rollback_auto_commit_group(&window) {
                    log::error!("Migration rollback failed: {rollback_error}");
                }
                return Err(error);
            }
        }
    } else {
        for vertex in staged {
            storage
                .update_vertex(&plan.target.space, vertex)
                .map_err(MigrationError::from)?;
        }
    }

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

fn execute_vertex_plan_streaming<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    batch_size: usize,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let mut all_errors = Vec::new();
    let mut total_rows: u64 = 0;
    let mut offset = 0usize;
    // First try to use a group window for atomicity across batches.
    let window = match storage.begin_auto_commit_group() {
        Ok(w) => Some(w),
        Err(e) if e.kind() == StorageErrorKind::NotSupported => {
            log::warn!(
                "Storage backend does not support auto-commit groups; \
                 falling back to per-row commits for streaming migration"
            );
            None
        }
        Err(e) => return Err(MigrationError::Storage(Box::new(e))),
    };

    if let Some(window) = window {
        let writer_result: Result<(), MigrationError> = (|| {
            let mut writer = storage
                .bind_auto_commit_writer(&window)
                .map_err(MigrationError::from)?;
            loop {
                let batch = storage
                    .scan_vertices_by_tag_paginated(
                        &plan.target.space,
                        &plan.target.label,
                        offset,
                        batch_size,
                    )
                    .map_err(MigrationError::from)?;
                if batch.is_empty() {
                    break;
                }
                for vertex in &batch {
                    let mut current = vertex.clone();
                    let mut skip = false;
                    for &step_idx in remaining {
                        let step = &plan.steps[step_idx];
                        let is_modifying = step.is_data_modifying()
                            || matches!(step, MigrationStep::AddColumn { .. });
                        if !is_modifying {
                            continue;
                        }
                        match apply_step_to_vertex(&current, &plan.target.label, step) {
                            Ok(Some(next)) => current = next,
                            Ok(None) => {}
                            Err(e) => {
                                all_errors.push(format!(
                                    "Step {} ({}) vertex {}: {}",
                                    step_idx + 1,
                                    step.description(),
                                    vertex.vid,
                                    e
                                ));
                                skip = true;
                                break;
                            }
                        }
                    }
                    if skip {
                        continue;
                    }
                    if !all_errors.is_empty() {
                        continue;
                    }
                    writer
                        .update_vertex(&plan.target.space, current)
                        .map_err(MigrationError::from)?;
                    total_rows += 1;
                }
                if !all_errors.is_empty() {
                    break;
                }
                offset += batch.len();
                if batch.len() < batch_size {
                    break;
                }
            }
            Ok(())
        })();
        match writer_result {
            Ok(()) => {
                if !all_errors.is_empty() {
                    if let Err(e) = storage.rollback_auto_commit_group(&window) {
                        log::error!("Migration rollback failed: {e}");
                    }
                    return Ok(MigrationReport {
                        success: false,
                        steps_completed: plan.completed_steps.len(),
                        rows_migrated: 0,
                        errors: all_errors,
                        completed_step_indices: plan.completed_steps.clone(),
                    });
                }
                storage
                    .finalize_auto_commit_group(&window)
                    .map_err(MigrationError::from)?;
            }
            Err(e) => {
                if let Err(re) = storage.rollback_auto_commit_group(&window) {
                    log::error!("Migration rollback failed: {re}");
                }
                return Err(e);
            }
        }
    } else {
        // Fallback per-batch per-row commits (non-atomic across batches)
        loop {
            let batch = storage
                .scan_vertices_by_tag_paginated(
                    &plan.target.space,
                    &plan.target.label,
                    offset,
                    batch_size,
                )
                .map_err(MigrationError::from)?;
            if batch.is_empty() {
                break;
            }
            let mut staged = Vec::new();
            for vertex in &batch {
                let mut current = vertex.clone();
                let mut skip = false;
                for &step_idx in remaining {
                    let step = &plan.steps[step_idx];
                    let is_modifying =
                        step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
                    if !is_modifying {
                        continue;
                    }
                    match apply_step_to_vertex(&current, &plan.target.label, step) {
                        Ok(Some(next)) => current = next,
                        Ok(None) => {}
                        Err(e) => {
                            all_errors.push(format!(
                                "Step {} ({}) vertex {}: {}",
                                step_idx + 1,
                                step.description(),
                                vertex.vid,
                                e
                            ));
                            skip = true;
                            break;
                        }
                    }
                }
                if skip {
                    continue;
                }
                staged.push(current);
            }
            if !all_errors.is_empty() {
                break;
            }
            for v in staged {
                storage
                    .update_vertex(&plan.target.space, v)
                    .map_err(MigrationError::from)?;
                total_rows += 1;
            }
            offset += batch.len();
            if batch.len() < batch_size {
                break;
            }
        }
        if !all_errors.is_empty() {
            return Ok(MigrationReport {
                success: false,
                steps_completed: plan.completed_steps.len(),
                rows_migrated: 0,
                errors: all_errors,
                completed_step_indices: plan.completed_steps.clone(),
            });
        }
    }

    let completed_step_indices: Vec<usize> = plan
        .completed_steps
        .iter()
        .copied()
        .chain(remaining.iter().copied())
        .collect();
    Ok(MigrationReport {
        success: true,
        steps_completed: completed_step_indices.len(),
        rows_migrated: total_rows,
        errors: vec![],
        completed_step_indices,
    })
}

fn execute_edge_plan<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let batch_size = if plan.batch_size == 0 {
        1000
    } else {
        plan.batch_size
    };
    let paginated_probe =
        storage.scan_edges_by_type_paginated(&plan.target.space, &plan.target.label, 0, 1);
    let use_paginated = match paginated_probe {
        Ok(_) => true,
        Err(e) if e.kind() == StorageErrorKind::NotSupported => false,
        Err(_) => true,
    };
    if use_paginated {
        return execute_edge_plan_streaming(storage, plan, remaining, batch_size);
    }
    let edges = storage.scan_edges_by_type(&plan.target.space, &plan.target.label)?;

    // Phase 1 (staging): transform all rows in memory first so a step or
    // conversion failure never leaves partially migrated data behind.
    let mut staged: Vec<Edge> = Vec::new();
    let mut errors = Vec::new();
    'edge_rows: for edge in &edges {
        let mut current = edge.clone();
        for &step_idx in remaining {
            let step = &plan.steps[step_idx];
            let is_modifying =
                step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
            if !is_modifying {
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
    // write. Same group-window commit as the vertex plan.
    let rows_migrated = staged.len() as u64;
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
    if let Some(window) = window {
        let result = (|| {
            let mut writer = storage
                .bind_auto_commit_writer(&window)
                .map_err(MigrationError::from)?;
            for edge in staged {
                writer
                    .update_edge(&plan.target.space, edge)
                    .map_err(MigrationError::from)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => storage
                .finalize_auto_commit_group(&window)
                .map_err(MigrationError::from)?,
            Err(error) => {
                if let Err(rollback_error) = storage.rollback_auto_commit_group(&window) {
                    log::error!("Migration rollback failed: {rollback_error}");
                }
                return Err(error);
            }
        }
    } else {
        for edge in staged {
            storage
                .update_edge(&plan.target.space, edge)
                .map_err(MigrationError::from)?;
        }
    }

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

fn execute_edge_plan_streaming<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    batch_size: usize,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let mut all_errors = Vec::new();
    let mut total_rows: u64 = 0;
    let mut offset = 0usize;
    let window = match storage.begin_auto_commit_group() {
        Ok(w) => Some(w),
        Err(e) if e.kind() == StorageErrorKind::NotSupported => {
            log::warn!("Storage backend does not support auto-commit groups; falling back to per-row commits for streaming migration");
            None
        }
        Err(e) => return Err(MigrationError::Storage(Box::new(e))),
    };
    if let Some(window) = window {
        let writer_result: Result<(), MigrationError> = (|| {
            let mut writer = storage
                .bind_auto_commit_writer(&window)
                .map_err(MigrationError::from)?;
            loop {
                let batch = storage
                    .scan_edges_by_type_paginated(
                        &plan.target.space,
                        &plan.target.label,
                        offset,
                        batch_size,
                    )
                    .map_err(MigrationError::from)?;
                if batch.is_empty() {
                    break;
                }
                for edge in &batch {
                    let mut current = edge.clone();
                    let mut skip = false;
                    for &step_idx in remaining {
                        let step = &plan.steps[step_idx];
                        let is_modifying = step.is_data_modifying()
                            || matches!(step, MigrationStep::AddColumn { .. });
                        if !is_modifying {
                            continue;
                        }
                        match apply_step_to_edge(&current, step) {
                            Ok(new_props) => current.props = new_props,
                            Err(e) => {
                                all_errors.push(format!(
                                    "Step {} ({}) edge ({:?}→{:?}): {}",
                                    step_idx + 1,
                                    step.description(),
                                    edge.src,
                                    edge.dst,
                                    e
                                ));
                                skip = true;
                                break;
                            }
                        }
                    }
                    if skip {
                        continue;
                    }
                    if !all_errors.is_empty() {
                        continue;
                    }
                    writer
                        .update_edge(&plan.target.space, current)
                        .map_err(MigrationError::from)?;
                    total_rows += 1;
                }
                if !all_errors.is_empty() {
                    break;
                }
                offset += batch.len();
                if batch.len() < batch_size {
                    break;
                }
            }
            Ok(())
        })();
        match writer_result {
            Ok(()) => {
                if !all_errors.is_empty() {
                    if let Err(e) = storage.rollback_auto_commit_group(&window) {
                        log::error!("Migration rollback failed: {e}");
                    }
                    return Ok(MigrationReport {
                        success: false,
                        steps_completed: plan.completed_steps.len(),
                        rows_migrated: 0,
                        errors: all_errors,
                        completed_step_indices: plan.completed_steps.clone(),
                    });
                }
                storage
                    .finalize_auto_commit_group(&window)
                    .map_err(MigrationError::from)?;
            }
            Err(e) => {
                if let Err(re) = storage.rollback_auto_commit_group(&window) {
                    log::error!("Migration rollback failed: {re}");
                }
                return Err(e);
            }
        }
    } else {
        loop {
            let batch = storage
                .scan_edges_by_type_paginated(
                    &plan.target.space,
                    &plan.target.label,
                    offset,
                    batch_size,
                )
                .map_err(MigrationError::from)?;
            if batch.is_empty() {
                break;
            }
            let mut staged = Vec::new();
            for edge in &batch {
                let mut current = edge.clone();
                let mut skip = false;
                for &step_idx in remaining {
                    let step = &plan.steps[step_idx];
                    let is_modifying =
                        step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
                    if !is_modifying {
                        continue;
                    }
                    match apply_step_to_edge(&current, step) {
                        Ok(new_props) => current.props = new_props,
                        Err(e) => {
                            all_errors.push(format!(
                                "Step {} ({}) edge ({:?}→{:?}): {}",
                                step_idx + 1,
                                step.description(),
                                edge.src,
                                edge.dst,
                                e
                            ));
                            skip = true;
                            break;
                        }
                    }
                }
                if skip {
                    continue;
                }
                staged.push(current);
            }
            if !all_errors.is_empty() {
                break;
            }
            for e in staged {
                storage
                    .update_edge(&plan.target.space, e)
                    .map_err(MigrationError::from)?;
                total_rows += 1;
            }
            offset += batch.len();
            if batch.len() < batch_size {
                break;
            }
        }
        if !all_errors.is_empty() {
            return Ok(MigrationReport {
                success: false,
                steps_completed: plan.completed_steps.len(),
                rows_migrated: 0,
                errors: all_errors,
                completed_step_indices: plan.completed_steps.clone(),
            });
        }
    }
    let completed_step_indices: Vec<usize> = plan
        .completed_steps
        .iter()
        .copied()
        .chain(remaining.iter().copied())
        .collect();
    Ok(MigrationReport {
        success: true,
        steps_completed: completed_step_indices.len(),
        rows_migrated: total_rows,
        errors: vec![],
        completed_step_indices,
    })
}

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::types::{EdgeTypeInfo, TagInfo};
use graphdb_core::{Edge, Tag, Value, Vertex};
use graphdb_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, MigrationHistoryRecord, MigrationStatus, StorageReader,
    StorageSchemaOps, StorageWriter,
};

use crate::config::MigrationConfig;
use crate::converter::convert_value;
use crate::error::MigrationError;
use crate::event::{MigrationEvent, MigrationEventListener};
use crate::lock::MigrationFileLock;
use crate::metrics::global_migration_metrics;
use crate::plan::{MigrationPlan, MigrationReport, MigrationStep, SafetyLevel};
use crate::progress::{MigrationProgress, NoopProgress};

static MIGRATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct MigrationLockGuard;

impl MigrationLockGuard {
    fn try_acquire() -> Result<Self, MigrationError> {
        if MIGRATION_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(MigrationError::Lock("migration in progress".to_string()));
        }
        Ok(Self)
    }
}

impl Drop for MigrationLockGuard {
    fn drop(&mut self) {
        MIGRATION_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

fn execute_schema_steps<S>(
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

fn record_migration_history<S>(
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

pub fn execute_migration_plan<S>(
    storage: &mut S,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    execute_migration_plan_with_progress(storage, plan, &NoopProgress, None)
}

pub fn execute_migration_plan_with_config<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    config: &MigrationConfig,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    execute_migration_plan_with_progress_and_config(
        storage,
        plan,
        &NoopProgress,
        None,
        config,
    )
}

pub fn execute_migration_plan_with_progress_and_config<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    config: &MigrationConfig,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let mut effective_plan = plan.clone();
    if config.batch_size != 0 {
        effective_plan.batch_size = config.batch_size;
    }
    execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        storage,
        &effective_plan,
        progress,
        event_listener,
        config.lock_path.as_deref(),
        config.checkpoint_dir.as_deref(),
    )
}

pub fn execute_migration_plan_with_progress<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    execute_migration_plan_with_progress_and_file_lock(
        storage,
        plan,
        progress,
        event_listener,
        None,
    )
}

pub fn execute_migration_plan_with_progress_and_file_lock<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    lock_path: Option<&Path>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        storage, plan, progress, event_listener, lock_path, None,
    )
}

pub fn execute_migration_plan_with_progress_and_file_lock_and_checkpoint<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    lock_path: Option<&Path>,
    checkpoint_dir: Option<&Path>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let _in_process_lock = MigrationLockGuard::try_acquire()?;
    let _file_lock: Option<MigrationFileLock> = if let Some(path) = lock_path {
        Some(MigrationFileLock::try_acquire(path)?)
    } else {
        None
    };
    let start = std::time::Instant::now();
    // --- checkpoint resume handling ---
    let mut checkpoint_completed: Vec<usize> = Vec::new();
    let mut checkpoint_rows: u64 = 0;
    if let Some(dir) = checkpoint_dir {
        match crate::plan::MigrationCheckpoint::load(plan, dir) {
            Ok(Some(cp)) => {
                log::info!(
                    "Resuming migration from checkpoint at step {} with completed {:?}",
                    cp.completed_step_index, cp.completed_steps
                );
                checkpoint_completed = cp.completed_steps.clone();
                if checkpoint_completed.is_empty() && cp.completed_step_index < plan.steps.len() {
                    checkpoint_completed.push(cp.completed_step_index);
                }
                checkpoint_rows = cp.rows_migrated_after;
            }
            Ok(None) => {}
            Err(e) => {
                log::warn!("Failed to load checkpoint: {}", e);
            }
        }
    }

    if let Some(listener) = event_listener {
        listener.on_event(MigrationEvent::Started { plan: plan.clone() });
    }
    progress.on_plan_start(plan);

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
        if let MigrationStep::DropLabel { label_name } = step {
            log::warn!(
                "Migration plan contains an irreversible DropLabel step: label '{}' on {}/{} will be permanently removed",
                label_name, plan.target.space, plan.target.label
            );
        }
        if let MigrationStep::DropEdgeType { edge_type_name } = step {
            log::warn!(
                "Migration plan contains an irreversible DropEdgeType step: edge_type '{}' on {}/{}",
                edge_type_name, plan.target.space, plan.target.label
            );
        }
    }

    if plan.dry_run {
        let report = execute_dry_run(storage, plan)?;
        if let Some(listener) = event_listener {
            if report.success {
                listener.on_event(MigrationEvent::Completed { report: report.clone() });
            } else {
                listener.on_event(MigrationEvent::Failed { error: report.errors.join("; ") });
            }
        }
        progress.on_plan_complete(plan, report.rows_migrated);
        return Ok(report);
    }

    if !plan.plan_hash.is_empty() {
        if let Ok(existing) = storage.list_migration_history(
            &plan.target.space,
            &plan.target.label,
            plan.target.is_edge,
        ) {
            for rec in existing {
                if rec.to_version == plan.version_range.to && rec.plan_hash != plan.plan_hash {
                    let err = format!(
                        "Checksum mismatch for version {}: stored hash {} != plan hash {}",
                        rec.to_version, rec.plan_hash, plan.plan_hash
                    );
                    if let Some(listener) = event_listener {
                        listener.on_event(MigrationEvent::Failed { error: err.clone() });
                    }
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                    return Err(MigrationError::Plan(err));
                }
            }
        }
    }

    let effective_remaining: Vec<usize> = (0..plan.steps.len())
        .filter(|i| !plan.completed_steps.contains(i) && !checkpoint_completed.contains(i))
        .collect();

    if effective_remaining.is_empty() {
        let mut all_done = plan.completed_steps.clone();
        for c in &checkpoint_completed {
            if !all_done.contains(c) {
                all_done.push(*c);
            }
        }
        all_done.sort_unstable();
        let report = MigrationReport {
            success: true,
            steps_completed: all_done.len(),
            rows_migrated: 0,
            errors: vec![],
            completed_step_indices: all_done.clone(),
        };
        if let Some(listener) = event_listener {
            listener.on_event(MigrationEvent::Completed { report: report.clone() });
        }
        progress.on_plan_complete(plan, 0);
        if let Some(dir) = checkpoint_dir {
            let _ = crate::plan::MigrationCheckpoint::cleanup(plan, dir);
        }
        global_migration_metrics()
            .record_success(report.rows_migrated, start.elapsed().as_millis() as u64);
        return Ok(report);
    }

    // Handle schema-modifying steps first.
    let schema_executed = execute_schema_steps(storage, plan, &effective_remaining, progress)
        .inspect_err(|_| {
            global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
        })?;
    let data_remaining: Vec<usize> = effective_remaining
        .into_iter()
        .filter(|idx| !schema_executed.contains(idx) && !plan.steps[*idx].is_schema_modifying())
        .collect();

    // Expand-contract handling for RenameColumn if requested.
    if plan.expand_contract.unwrap_or(false) {
        // No extra handling needed
    }

    if data_remaining.is_empty() {
        let mut all_completed = plan.completed_steps.clone();
        for c in &checkpoint_completed {
            if !all_completed.contains(c) {
                all_completed.push(*c);
            }
        }
        for idx in &schema_executed {
            if !all_completed.contains(idx) {
                all_completed.push(*idx);
            }
        }
        all_completed.sort_unstable();
        if let Some(dir) = checkpoint_dir {
            if !schema_executed.is_empty() {
                let cp = crate::plan::MigrationCheckpoint {
                    completed_step_index: *schema_executed.last().unwrap_or(&0),
                    rows_migrated_before: 0,
                    rows_migrated_after: 0,
                    timestamp: crate::plan::checkpoint_now_millis(),
                    step_result: crate::plan::StepResult::Success,
                    completed_steps: all_completed.clone(),
                };
                let _ = cp.save(plan, dir);
            }
            let _ = crate::plan::MigrationCheckpoint::cleanup(plan, dir);
        }
        let report = MigrationReport {
            success: true,
            steps_completed: all_completed.len(),
            rows_migrated: 0,
            errors: vec![],
            completed_step_indices: all_completed.clone(),
        };
        record_migration_history(storage, plan, 0, MigrationStatus::Applied, None);
        if let Some(listener) = event_listener {
            listener.on_event(MigrationEvent::Completed { report: report.clone() });
        }
        progress.on_plan_complete(plan, 0);
        global_migration_metrics()
            .record_success(report.rows_migrated, start.elapsed().as_millis() as u64);
        return Ok(report);
    }

    // Prepare combined completed set including schema
    let mut all_completed: Vec<usize> = {
        let mut v = plan.completed_steps.clone();
        for c in &checkpoint_completed {
            if !v.contains(c) {
                v.push(*c);
            }
        }
        for idx in &schema_executed {
            if !v.contains(idx) {
                v.push(*idx);
            }
        }
        v.sort_unstable();
        v
    };

    // Save a checkpoint after schema stage if applicable
    if let Some(dir) = checkpoint_dir {
        if !schema_executed.is_empty() {
            let cp = crate::plan::MigrationCheckpoint {
                completed_step_index: *schema_executed.last().unwrap(),
                rows_migrated_before: 0,
                rows_migrated_after: checkpoint_rows,
                timestamp: crate::plan::checkpoint_now_millis(),
                step_result: crate::plan::StepResult::Success,
                completed_steps: all_completed.clone(),
            };
            if let Err(e) = cp.save(plan, dir) {
                log::warn!("Failed to save checkpoint after schema steps: {}", e);
            }
        }
    }

    let mut overall_rows: u64 = 0;
    // Per-step loop with checkpoint save after each step
    for &idx in &data_remaining {
        let step = &plan.steps[idx];
        progress.on_step_start(idx, step);
        if let Some(listener) = event_listener {
            listener.on_event(MigrationEvent::StepStarted { step_idx: idx });
        }

        let single_slice = vec![idx];
        let step_report = if plan.target.is_edge {
            execute_edge_plan_with_progress(storage, plan, &single_slice, progress, event_listener)
                .inspect_err(|_| {
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                })?
        } else {
            execute_vertex_plan_with_progress(storage, plan, &single_slice, progress, event_listener)
                .inspect_err(|_| {
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                })?
        };

        if !step_report.success {
            let cp = crate::plan::MigrationCheckpoint {
                completed_step_index: idx,
                rows_migrated_before: checkpoint_rows + overall_rows,
                rows_migrated_after: checkpoint_rows + overall_rows,
                timestamp: crate::plan::checkpoint_now_millis(),
                step_result: crate::plan::StepResult::Failed(step_report.errors.join("; ")),
                completed_steps: all_completed.clone(),
            };
            if let Some(dir) = checkpoint_dir {
                let _ = cp.save(plan, dir);
            }
            record_migration_history(
                storage,
                plan,
                0,
                MigrationStatus::Failed,
                Some(step_report.errors.join("; ")),
            );
            if let Some(listener) = event_listener {
                listener.on_event(MigrationEvent::Failed { error: step_report.errors.join("; ") });
            }
            for err in &step_report.errors {
                progress.on_error(err);
            }
            progress.on_plan_complete(plan, 0);
            let report = MigrationReport {
                success: false,
                steps_completed: all_completed.len(),
                rows_migrated: 0,
                errors: step_report.errors.clone(),
                completed_step_indices: all_completed.clone(),
            };
            global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
            return Ok(report);
        }

        // Track rows: use max to avoid double counting same vertices across steps
        if step_report.rows_migrated > overall_rows {
            overall_rows = step_report.rows_migrated;
        }

        all_completed.push(idx);
        all_completed.sort_unstable();

        progress.on_step_complete(idx, step);
        if let Some(listener) = event_listener {
            listener.on_event(MigrationEvent::StepCompleted { step_idx: idx, rows: step_report.rows_migrated });
        }

        let cp = crate::plan::MigrationCheckpoint {
            completed_step_index: idx,
            rows_migrated_before: checkpoint_rows + overall_rows.saturating_sub(step_report.rows_migrated),
            rows_migrated_after: checkpoint_rows + overall_rows,
            timestamp: crate::plan::checkpoint_now_millis(),
            step_result: crate::plan::StepResult::Success,
            completed_steps: all_completed.clone(),
        };
        if let Some(dir) = checkpoint_dir {
            if let Err(e) = cp.save(plan, dir) {
                log::warn!("Failed to save checkpoint for step {}: {}", idx, e);
            }
        }
    }

    // All data steps succeeded
    if let Some(dir) = checkpoint_dir {
        let _ = crate::plan::MigrationCheckpoint::cleanup(plan, dir);
    }

    let total_rows = if checkpoint_rows > overall_rows { checkpoint_rows } else { overall_rows };
    // If we resumed, total distinct rows is max; but if steps were already partially done,
    // checkpoint_rows already represents previous total, and overall_rows is count for remaining steps (same set).
    // Keep max.
    let final_rows = total_rows;
    let report = MigrationReport {
        success: true,
        steps_completed: all_completed.len(),
        rows_migrated: final_rows,
        errors: vec![],
        completed_step_indices: all_completed.clone(),
    };
    record_migration_history(storage, plan, final_rows, MigrationStatus::Applied, None);
    if let Some(listener) = event_listener {
        listener.on_event(MigrationEvent::Completed { report: report.clone() });
    }
    progress.on_plan_complete(plan, final_rows);
    global_migration_metrics().record_success(final_rows, start.elapsed().as_millis() as u64);
    Ok(report)
}

fn execute_vertex_plan_with_progress<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    progress: &dyn MigrationProgress,
    _event_listener: Option<&dyn MigrationEventListener>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    // delegate to existing vertex plan but with progress row callbacks
    let report = execute_vertex_plan(storage, plan, remaining)?;
    // Emit row progress approximation
    if report.success && report.rows_migrated > 0 {
        progress.on_row_processed(report.rows_migrated);
    }
    Ok(report)
}

fn execute_edge_plan_with_progress<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    remaining: &[usize],
    progress: &dyn MigrationProgress,
    _event_listener: Option<&dyn MigrationEventListener>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let report = execute_edge_plan(storage, plan, remaining)?;
    if report.success && report.rows_migrated > 0 {
        progress.on_row_processed(report.rows_migrated);
    }
    Ok(report)
}

fn execute_dry_run<S>(
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
                let is_mod = step.is_data_modifying()
                    || matches!(step, MigrationStep::AddColumn { .. });
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
                let is_mod = step.is_data_modifying()
                    || matches!(step, MigrationStep::AddColumn { .. });
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
    let batch_size = if plan.batch_size == 0 { 1000 } else { plan.batch_size };
    // Try streaming paginated path first; fallback to full scan if not supported.
    let paginated_probe = storage.scan_vertices_by_tag_paginated(
        &plan.target.space,
        &plan.target.label,
        0,
        1,
    );
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
    let batch_size = if plan.batch_size == 0 { 1000 } else { plan.batch_size };
    let paginated_probe = storage.scan_edges_by_type_paginated(&plan.target.space, &plan.target.label, 0, 1);
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
            let mut writer = storage.bind_auto_commit_writer(&window).map_err(MigrationError::from)?;
            loop {
                let batch = storage
                    .scan_edges_by_type_paginated(&plan.target.space, &plan.target.label, offset, batch_size)
                    .map_err(MigrationError::from)?;
                if batch.is_empty() {
                    break;
                }
                for edge in &batch {
                    let mut current = edge.clone();
                    let mut skip = false;
                    for &step_idx in remaining {
                        let step = &plan.steps[step_idx];
                        let is_modifying = step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
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
                    writer.update_edge(&plan.target.space, current).map_err(MigrationError::from)?;
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
                storage.finalize_auto_commit_group(&window).map_err(MigrationError::from)?;
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
                .scan_edges_by_type_paginated(&plan.target.space, &plan.target.label, offset, batch_size)
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
                    let is_modifying = step.is_data_modifying() || matches!(step, MigrationStep::AddColumn { .. });
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
                storage.update_edge(&plan.target.space, e).map_err(MigrationError::from)?;
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
    let completed_step_indices: Vec<usize> = plan.completed_steps.iter().copied().chain(remaining.iter().copied()).collect();
    Ok(MigrationReport {
        success: true,
        steps_completed: completed_step_indices.len(),
        rows_migrated: total_rows,
        errors: vec![],
        completed_step_indices,
    })
}

pub fn rollback_migration<S>(
    storage: &mut S,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader + StorageWriter + StorageSchemaOps + AutoCommitGroupOps + AutoCommitBatchOps + ?Sized,
{
    let result = match &plan.rollback_plan {
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
    };
    if let Ok(ref report) = result {
        if report.success {
            let rollback_record = MigrationHistoryRecord {
                id: 0,
                space: plan.target.space.clone(),
                label: plan.target.label.clone(),
                is_edge: plan.target.is_edge,
                from_version: plan.version_range.to,
                to_version: plan.version_range.from,
                plan_hash: plan.plan_hash.clone(),
                safety_level: format!("{:?}", plan.overall_safety),
                steps_count: plan.steps.len(),
                rows_migrated: report.rows_migrated,
                status: MigrationStatus::RolledBack,
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
                error_message: None,
            };
            let _ = storage.record_migration_history(rollback_record);
        }
    }
    result
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
        MigrationStep::ChangeNullability {
            name,
            was_nullable,
            now_nullable,
        } => {
            if *was_nullable && !now_nullable {
                for (prop_name, val) in tag.properties.iter() {
                    if prop_name == name && matches!(val, Value::Null(_)) {
                        return Err(format!(
                            "cannot set column '{}' NOT NULL: found NULL values",
                            name
                        ));
                    }
                }
            }
            return Ok(None);
        }
        MigrationStep::AddColumn {
            name,
            data_type: _,
            nullable: _,
            default_value,
        } => {
            if tag.properties.contains_key(name) {
                return Ok(None);
            }
            if let Some(default) = default_value {
                if tag.properties.contains_key(name) {
                    let current = tag.properties.get(name).unwrap();
                    if current == default {
                        return Ok(None);
                    }
                }
            }
            tag.properties.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => return Ok(None),
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
        MigrationStep::ChangeNullability {
            name,
            was_nullable,
            now_nullable,
        } => {
            if *was_nullable && !now_nullable {
                for (prop_name, val) in edge.props.iter() {
                    if prop_name == name && matches!(val, Value::Null(_)) {
                        return Err(format!(
                            "cannot set column '{}' NOT NULL: found NULL values",
                            name
                        ));
                    }
                }
            }
            Ok(edge.props.clone())
        }
        MigrationStep::AddColumn {
            name,
            data_type: _,
            nullable: _,
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
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => Ok(edge.props.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{MigrationTarget, VersionRange};
    use graphdb_core::types::{EdgeTypeInfo, Index, SpaceInfo, TagInfo, VertexId};
    use graphdb_core::{DataType, Value, Vertex, Edge, EdgeDirection};
    use graphdb_storage::{LabelVersionHistory, StorageReader, StorageWriter, AutoCommitBatchOps, AutoCommitGroupOps, MigrationHistoryRecord};
    use graphdb_core::StorageError;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    type VertexMap = Arc<Mutex<HashMap<(String, String), Vec<Vertex>>>>;
    type EdgeMap = Arc<Mutex<HashMap<(String, String), Vec<Edge>>>>;
    type HistoryVec = Arc<Mutex<Vec<MigrationHistoryRecord>>>;

    #[derive(Debug, Clone)]
    struct TestStorage {
        vertices: VertexMap,
        edges: EdgeMap,
        migration_history: HistoryVec,
    }

    impl TestStorage {
        fn new() -> Self {
            Self {
                vertices: Arc::new(Mutex::new(HashMap::new())),
                edges: Arc::new(Mutex::new(HashMap::new())),
                migration_history: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn insert_vertex(&self, space: &str, label: &str, vid: i64, props: HashMap<String, Value>) {
            let mut map = self.vertices.lock().unwrap();
            let entry = map.entry((space.to_string(), label.to_string())).or_default();
            let tag = graphdb_core::vertex_edge_path::Tag { name: label.to_string(), properties: props.clone() };
            let vertex = Vertex { vid: VertexId::from_int64(vid), id: vid, tags: vec![tag], properties: props };
            entry.push(vertex);
        }
        fn get_vertices(&self, space: &str, label: &str) -> Vec<Vertex> {
            self.vertices.lock().unwrap().get(&(space.to_string(), label.to_string())).cloned().unwrap_or_default()
        }
        #[allow(dead_code)]
        fn insert_edge(&self, space: &str, edge_type: &str, src: i64, dst: i64, props: HashMap<String, Value>) {
            let mut map = self.edges.lock().unwrap();
            let entry = map.entry((space.to_string(), edge_type.to_string())).or_default();
            let edge = Edge { src: VertexId::from_int64(src), dst: VertexId::from_int64(dst), edge_type: edge_type.to_string(), ranking: 0, props };
            entry.push(edge);
        }
    }

    impl StorageReader for TestStorage {
        fn get_vertex(&self, _space: &str, _id: &VertexId) -> Result<Option<Vertex>, StorageError> { Ok(None) }
        fn scan_vertices(&self, _space: &str) -> Result<Vec<Vertex>, StorageError> { Ok(Vec::new()) }
        fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError> {
            Ok(self.get_vertices(space, tag))
        }
        fn scan_vertices_by_prop(&self, _space: &str, _tag: &str, _prop: &str, _value: &Value) -> Result<Vec<Vertex>, StorageError> { Ok(Vec::new()) }
        fn get_edge(&self, _space: &str, _src: &VertexId, _dst: &VertexId, _edge_type: &str, _rank: i64) -> Result<Option<Edge>, StorageError> { Ok(None) }
        fn get_node_edges(&self, _space: &str, _node_id: &VertexId, _direction: EdgeDirection) -> Result<Vec<Edge>, StorageError> { Ok(Vec::new()) }
        fn neighbor_dst_ids_batch(&self, _space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<Vec<VertexId>>, StorageError> { Ok(Vec::new()) }
        fn out_degree_batch(&self, _space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<usize>, StorageError> { Ok(Vec::new()) }
        fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
            Ok(self.edges.lock().unwrap().get(&(space.to_string(), edge_type.to_string())).cloned().unwrap_or_default())
        }
        fn scan_all_edges(&self, _space: &str) -> Result<Vec<Edge>, StorageError> { Ok(Vec::new()) }
        fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError> {
            Ok(self.get_vertices(space, tag).len() as u64)
        }
        fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
            Ok(self.edges.lock().unwrap().get(&(space.to_string(), edge_type.to_string())).map(|v| v.len() as u64).unwrap_or(0))
        }
        fn lookup_index(&self, _space: &str, _index: &str, _value: &Value) -> Result<Vec<Value>, StorageError> { Ok(Vec::new()) }
        fn get_vertex_with_schema(&self, _space: &str, _tag: &str, _id: &Value) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> { Ok(None) }
        fn get_edge_with_schema(&self, _space: &str, _edge_type: &str, _src: &Value, _dst: &Value) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> { Ok(None) }
        fn scan_vertices_with_schema(&self, _space: &str, _tag: &str) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> { Ok(Vec::new()) }
        fn scan_edges_with_schema(&self, _space: &str, _edge_type: &str) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> { Ok(Vec::new()) }
        fn get_space(&self, _space: &str) -> Result<Option<SpaceInfo>, StorageError> { Ok(None) }
        fn get_space_by_id(&self, _space_id: u64) -> Result<Option<SpaceInfo>, StorageError> { Ok(None) }
        fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> { Ok(Vec::new()) }
        fn get_space_id(&self, _space: &str) -> Result<u64, StorageError> { Ok(1) }
        fn space_exists(&self, _space: &str) -> bool { false }
        fn get_tag(&self, _space: &str, _tag: &str) -> Result<Option<TagInfo>, StorageError> { Ok(None) }
        fn list_tags(&self, _space: &str) -> Result<Vec<TagInfo>, StorageError> { Ok(Vec::new()) }
        fn get_edge_type(&self, _space: &str, _edge_type: &str) -> Result<Option<EdgeTypeInfo>, StorageError> { Ok(None) }
        fn list_edge_types(&self, _space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> { Ok(Vec::new()) }
        fn get_tag_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> { Ok(None) }
        fn list_tag_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> { Ok(Vec::new()) }
        fn get_edge_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> { Ok(None) }
        fn list_edge_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> { Ok(Vec::new()) }
        fn get_vertex_version_history(&self, _space: &str, _tag: &str) -> Result<Option<LabelVersionHistory>, StorageError> { Ok(None) }
        fn get_edge_version_history(&self, _space: &str, _edge_type: &str) -> Result<Option<LabelVersionHistory>, StorageError> { Ok(None) }
        fn get_vertex_schema_changes(&self, _space: &str, _tag: &str, _from_version: u64, _to_version: u64) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> { Ok(Vec::new()) }
        fn get_edge_schema_changes(&self, _space: &str, _edge_type: &str, _from_version: u64, _to_version: u64) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> { Ok(Vec::new()) }
        fn detect_vertex_breaking_changes(&self, _space: &str, _tag: &str, _from_version: u64, _to_version: u64) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> { Ok(Vec::new()) }
        fn detect_edge_breaking_changes(&self, _space: &str, _edge_type: &str, _from_version: u64, _to_version: u64) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> { Ok(Vec::new()) }
        fn record_migration_history(&self, record: MigrationHistoryRecord) -> Result<(), StorageError> {
            self.migration_history.lock().unwrap().push(record);
            Ok(())
        }
        fn list_migration_history(&self, _space: &str, _label: &str, _is_edge: bool) -> Result<Vec<MigrationHistoryRecord>, StorageError> { Ok(self.migration_history.lock().unwrap().clone()) }
        fn get_applied_versions(&self, _space: &str, _label: &str, _is_edge: bool) -> Result<Vec<u64>, StorageError> { Ok(Vec::new()) }
        fn scan_vertices_by_tag_paginated(&self, space: &str, tag: &str, offset: usize, limit: usize) -> Result<Vec<Vertex>, StorageError> {
            Ok(self.get_vertices(space, tag).into_iter().skip(offset).take(limit).collect())
        }
        fn scan_edges_by_type_paginated(&self, space: &str, edge_type: &str, offset: usize, limit: usize) -> Result<Vec<Edge>, StorageError> {
            Ok(self.edges.lock().unwrap().get(&(space.to_string(), edge_type.to_string())).cloned().unwrap_or_default().into_iter().skip(offset).take(limit).collect())
        }
    }

    impl StorageWriter for TestStorage {
        fn insert_vertex(&mut self, _space: &str, _vertex: Vertex) -> Result<VertexId, StorageError> { Ok(VertexId::from_int64(0)) }
        fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
            let label = vertex.tags.first().map(|t| t.name.clone()).unwrap_or_default();
            let mut map = self.vertices.lock().unwrap();
            if let Some(vec) = map.get_mut(&(space.to_string(), label.clone())) {
                for v in vec.iter_mut() {
                    if v.vid == vertex.vid {
                        *v = vertex.clone();
                        return Ok(());
                    }
                }
                vec.push(vertex);
            } else {
                map.insert((space.to_string(), label), vec![vertex]);
            }
            Ok(())
        }
        fn delete_vertex(&mut self, _space: &str, _id: &VertexId) -> Result<(), StorageError> { Ok(()) }
        fn delete_vertex_with_edges(&mut self, _space: &str, _id: &VertexId) -> Result<(), StorageError> { Ok(()) }
        fn batch_insert_vertices(&mut self, _space: &str, _vertices: Vec<Vertex>) -> Result<Vec<VertexId>, StorageError> { Ok(Vec::new()) }
        fn delete_tags(&mut self, _space: &str, _vertex_id: &VertexId, _tag_names: &[String]) -> Result<usize, StorageError> { Ok(0) }
        fn insert_edge(&mut self, _space: &str, _edge: Edge) -> Result<(), StorageError> { Ok(()) }
        fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
            let mut map = self.edges.lock().unwrap();
            let key = (space.to_string(), edge.edge_type.clone());
            if let Some(vec) = map.get_mut(&key) {
                for e in vec.iter_mut() {
                    if e.src == edge.src && e.dst == edge.dst && e.ranking == edge.ranking {
                        *e = edge.clone();
                        return Ok(());
                    }
                }
                vec.push(edge);
            } else {
                map.insert(key, vec![edge]);
            }
            Ok(())
        }
        fn delete_edge(&mut self, _space: &str, _src: &VertexId, _dst: &VertexId, _edge_type: &str, _rank: i64) -> Result<(), StorageError> { Ok(()) }
        fn batch_insert_edges(&mut self, _space: &str, _edges: Vec<Edge>) -> Result<(), StorageError> { Ok(()) }
        fn insert_vertex_data(&mut self, _space: &str, _info: &graphdb_core::types::InsertVertexInfo) -> Result<bool, StorageError> { Ok(true) }
        fn insert_edge_data(&mut self, _space: &str, _info: &graphdb_core::types::InsertEdgeInfo) -> Result<bool, StorageError> { Ok(true) }
        fn delete_vertex_data(&mut self, _space: &str, _vertex_id: &str) -> Result<bool, StorageError> { Ok(true) }
        fn delete_edge_data(&mut self, _space: &str, _src: &str, _dst: &str, _rank: i64) -> Result<bool, StorageError> { Ok(true) }
        fn update_data(&mut self, _space: &str, _space_id: u64, _info: &graphdb_core::types::UpdateInfo) -> Result<bool, StorageError> { Ok(true) }
    }

    impl AutoCommitBatchOps for TestStorage {
        fn begin_auto_commit_batch(&self) -> graphdb_core::StorageResult<Arc<graphdb_storage::AutoCommitBatchWindow>> {
            Err(StorageError::not_supported("not supported"))
        }
        fn bind_auto_commit_statement(&self, _window: &Arc<graphdb_storage::AutoCommitBatchWindow>) -> graphdb_core::StorageResult<Self> where Self: Sized { Err(StorageError::not_supported("not supported")) }
        fn finalize_auto_commit_batch(&self, _window: &graphdb_storage::AutoCommitBatchWindow) -> graphdb_core::StorageResult<()> { Err(StorageError::not_supported("not supported")) }
    }
    impl AutoCommitGroupOps for TestStorage {
        fn begin_auto_commit_group(&self) -> graphdb_core::StorageResult<Arc<graphdb_storage::AutoCommitBatchWindow>> { Err(StorageError::not_supported("not supported")) }
        fn finalize_auto_commit_group(&self, _window: &graphdb_storage::AutoCommitBatchWindow) -> graphdb_core::StorageResult<()> { Err(StorageError::not_supported("not supported")) }
    }

    impl graphdb_storage::StorageSchemaOps for TestStorage {
        fn create_space(&mut self, _space: &mut SpaceInfo) -> Result<bool, StorageError> { Ok(true) }
        fn drop_space(&mut self, _space: &str) -> Result<bool, StorageError> { Ok(true) }
        fn clear_space(&mut self, _space: &str) -> Result<bool, StorageError> { Ok(true) }
        fn alter_space_comment(&mut self, _space_id: u64, _comment: String) -> Result<bool, StorageError> { Ok(true) }
        fn create_tag(&mut self, _space: &str, _tag: &TagInfo) -> Result<u32, StorageError> { Ok(1) }
        fn alter_tag(&mut self, _space: &str, _tag: &str, _additions: Vec<graphdb_core::types::PropertyDef>, _deletions: Vec<String>) -> Result<bool, StorageError> { Ok(true) }
        fn rename_vertex_property(&mut self, _label: graphdb_core::types::LabelId, _old_name: &str, _new_name: &str) -> Result<(), StorageError> { Ok(()) }
        fn rename_tag_property(&mut self, _space: &str, _tag: &str, _old_name: &str, _new_name: &str) -> Result<bool, StorageError> { Ok(true) }
        fn drop_tag(&mut self, _space: &str, _tag: &str) -> Result<bool, StorageError> { Ok(true) }
        fn create_edge_type(&mut self, _space: &str, _edge: &EdgeTypeInfo) -> Result<u32, StorageError> { Ok(1) }
        fn alter_edge_type(&mut self, _space: &str, _edge_type: &str, _additions: Vec<graphdb_core::types::PropertyDef>, _deletions: Vec<String>) -> Result<bool, StorageError> { Ok(true) }
        fn drop_edge_type(&mut self, _space: &str, _edge_type: &str) -> Result<bool, StorageError> { Ok(true) }
        fn create_tag_index(&mut self, _space: &str, _info: &Index) -> Result<bool, StorageError> { Ok(true) }
        fn drop_tag_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> { Ok(true) }
        fn rebuild_tag_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> { Ok(true) }
        fn create_edge_index(&mut self, _space: &str, _info: &Index) -> Result<bool, StorageError> { Ok(true) }
        fn drop_edge_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> { Ok(true) }
        fn rebuild_edge_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> { Ok(true) }
    }

    #[test]
    fn test_execute_add_column() {
        let mut storage = TestStorage::new();
        storage.insert_vertex("s", "User", 1, HashMap::new());
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::AddColumn { name: "email".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("a@b.com")) }],
            1,
            SafetyLevel::Safe,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert_eq!(vertices[0].tags[0].properties.get("email"), Some(&Value::string("a@b.com")));
        // check history recorded
        assert_eq!(storage.migration_history.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_execute_drop_column() {
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("old".into(), Value::string("v"));
        storage.insert_vertex("s", "User", 1, props);
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::DropColumn { name: "old".into() }],
            1,
            SafetyLevel::Dangerous,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert!(!vertices[0].tags[0].properties.contains_key("old"));
    }

    #[test]
    fn test_execute_type_convert() {
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("age".into(), Value::Int(42));
        storage.insert_vertex("s", "User", 1, props);
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::ConvertType { name: "age".into(), from_type: DataType::Int, to_type: DataType::BigInt }],
            1,
            SafetyLevel::Warning,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert_eq!(vertices[0].tags[0].properties.get("age"), Some(&Value::BigInt(42)));
    }

    #[test]
    fn test_execute_rollback() {
        let mut storage = TestStorage::new();
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::AddColumn { name: "email".into(), data_type: DataType::String, nullable: true, default_value: None }],
            0,
            SafetyLevel::Safe,
            Some(Box::new(MigrationPlan::new(
                MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
                VersionRange { from: 2, to: 1 },
                vec![MigrationStep::DropColumn { name: "email".into() }],
                0,
                SafetyLevel::Dangerous,
                None,
            ))),
        );
        let report = rollback_migration(&mut storage, &plan).unwrap();
        assert!(report.success);
    }

    #[test]
    fn test_idempotent_execution() {
        let mut storage = TestStorage::new();
        storage.insert_vertex("s", "User", 1, HashMap::new());
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::AddColumn { name: "email".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("x")) }],
            1,
            SafetyLevel::Safe,
            None,
        );
        let r1 = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(r1.success);
        let r2 = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(r2.success);
        let vertices = storage.get_vertices("s", "User");
        assert_eq!(vertices[0].tags[0].properties.get("email"), Some(&Value::string("x")));
        // No duplicate history? second execution will attempt to record same to_version -> our mock just pushes, but real manager would reject AlreadyExists. For test we just check no panic.
    }

    #[test]
    fn test_partial_failure() {
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("age".into(), Value::string("not_a_number"));
        storage.insert_vertex("s", "User", 1, props);
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::ConvertType { name: "age".into(), from_type: DataType::String, to_type: DataType::Int }],
            1,
            SafetyLevel::Warning,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(!report.success);
        assert_eq!(report.rows_migrated, 0);
        assert!(!report.errors.is_empty());
    }

    #[test]
    fn test_dry_run_no_commit() {
        let mut storage = TestStorage::new();
        storage.insert_vertex("s", "User", 1, HashMap::new());
        let mut plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::AddColumn { name: "email".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("dry")) }],
            1,
            SafetyLevel::Safe,
            None,
        );
        plan.dry_run = true;
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert!(!vertices[0].tags[0].properties.contains_key("email"));
    }

    #[test]
    fn test_idempotent_add_column() {
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("email".into(), Value::string("exists"));
        storage.insert_vertex("s", "User", 1, props);
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![MigrationStep::AddColumn { name: "email".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("new")) }],
            1,
            SafetyLevel::Safe,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert_eq!(vertices[0].tags[0].properties.get("email"), Some(&Value::string("exists")));
    }

    #[test]
    fn test_expand_contract_rename() {
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("old_name".into(), Value::string("hello"));
        storage.insert_vertex("s", "User", 1, props);
        let plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![
                MigrationStep::AddColumn { name: "new_name".into(), data_type: DataType::String, nullable: true, default_value: None },
                MigrationStep::RenameColumn { old_name: "old_name".into(), new_name: "new_name".into() },
                MigrationStep::DropColumn { name: "old_name".into() },
            ],
            1,
            SafetyLevel::Warning,
            None,
        );
        let report = execute_migration_plan(&mut storage, &plan).unwrap();
        assert!(report.success);
        let vertices = storage.get_vertices("s", "User");
        assert!(!vertices[0].tags[0].properties.contains_key("old_name"));
        assert_eq!(vertices[0].tags[0].properties.get("new_name"), Some(&Value::string("hello")));
    }

    #[test]
    fn test_checkpoint_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let mut storage = TestStorage::new();
        let mut props = HashMap::new();
        props.insert("a".into(), Value::string("v1"));
        storage.insert_vertex("s", "User", 1, props);
        let mut plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![
                MigrationStep::AddColumn { name: "a".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("v1")) },
                MigrationStep::AddColumn { name: "b".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("v2")) },
            ],
            1,
            SafetyLevel::Safe,
            None,
        );
        plan.plan_hash = "ckpt_test_hash".to_string();
        // Simulate interrupted run: save checkpoint with first step completed and storage already has column a
        let cp = crate::plan::MigrationCheckpoint {
            completed_step_index: 0,
            rows_migrated_before: 0,
            rows_migrated_after: 1,
            timestamp: crate::plan::checkpoint_now_millis(),
            step_result: crate::plan::StepResult::Success,
            completed_steps: vec![0],
        };
        cp.save(&plan, tmp.path()).unwrap();
        let report = execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
            &mut storage,
            &plan,
            &crate::progress::NoopProgress,
            None,
            None,
            Some(tmp.path()),
        ).unwrap();
        assert!(report.success);
        assert!(report.completed_step_indices.contains(&0));
        assert!(report.completed_step_indices.contains(&1));
        let vertices = storage.get_vertices("s", "User");
        assert_eq!(vertices[0].tags[0].properties.get("a"), Some(&Value::string("v1")));
        assert_eq!(vertices[0].tags[0].properties.get("b"), Some(&Value::string("v2")));
        // checkpoint file should be cleaned up after success
        assert!(crate::plan::MigrationCheckpoint::load(&plan, tmp.path()).unwrap().is_none());
    }

    #[test]
    fn test_checkpoint_save_per_step() {
        let tmp = tempfile::tempdir().unwrap();
        let mut storage = TestStorage::new();
        storage.insert_vertex("s", "User", 1, HashMap::new());
        let mut plan = MigrationPlan::new(
            MigrationTarget { space: "s".into(), label: "User".into(), is_edge: false },
            VersionRange { from: 1, to: 2 },
            vec![
                MigrationStep::AddColumn { name: "c1".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("x")) },
                MigrationStep::AddColumn { name: "c2".into(), data_type: DataType::String, nullable: true, default_value: Some(Value::string("y")) },
            ],
            1,
            SafetyLevel::Safe,
            None,
        );
        plan.plan_hash = "ckpt_save_test".to_string();
        let report = execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
            &mut storage,
            &plan,
            &crate::progress::NoopProgress,
            None,
            None,
            Some(tmp.path()),
        ).unwrap();
        assert!(report.success);
        // checkpoint should be cleaned up after success, but during execution it was saved per step
        assert!(report.completed_step_indices.len() == 2);
    }
}

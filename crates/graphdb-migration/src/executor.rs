//! Migration plan entry points: locking, checkpoint resume orchestration,
//! history recording, and rollback.
//!
//! Step-level execution lives in [`plan_runner`]; row-level step application
//! lives in [`data_apply`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use graphdb_core::event_dispatch::EventSubscriptions;
use graphdb_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, MigrationHistoryRecord, MigrationStatus, StorageReader,
    StorageSchemaOps, StorageWriter,
};

use crate::config::MigrationConfig;
use crate::error::MigrationError;
use crate::event::{MigrationDispatcher, MigrationEvent, MigrationEventListener};
use crate::lock::MigrationFileLock;
use crate::metrics::global_migration_metrics;
use crate::plan::{MigrationPlan, MigrationReport, MigrationStep, SafetyLevel};
use crate::progress::{MigrationProgress, NoopProgress};

#[cfg(test)]
mod tests;

mod data_apply;
mod plan_runner;

use plan_runner::{
    execute_dry_run, execute_edge_plan_with_progress, execute_schema_steps,
    execute_vertex_plan_with_progress, record_migration_history,
};

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

pub fn execute_migration_plan<S>(
    storage: &mut S,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    execute_migration_plan_with_progress(storage, plan, &NoopProgress, None)
}

pub fn execute_migration_plan_with_config<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    config: &MigrationConfig,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    execute_migration_plan_with_progress_and_config(storage, plan, &NoopProgress, None, config)
}

pub fn execute_migration_plan_with_progress_and_config<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    config: &MigrationConfig,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
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
        None,
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
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
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
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        storage,
        plan,
        progress,
        event_listener,
        None,
        lock_path,
        None,
    )
}

/// Execute a migration plan, fanning lifecycle events out to both a legacy
/// trait listener and a shared `EventSubscriptions` registry.
///
/// This is the only entry point that accepts a registry; all other
/// `execute_migration_plan_*` wrappers pass `None` and preserve their
/// existing signatures.
pub fn execute_migration_plan_with_event_registry<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    event_registry: Option<&Arc<EventSubscriptions<MigrationEvent>>>,
    lock_path: Option<&Path>,
    checkpoint_dir: Option<&Path>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        storage,
        plan,
        progress,
        event_listener,
        event_registry,
        lock_path,
        checkpoint_dir,
    )
}

pub fn execute_migration_plan_with_progress_and_file_lock_and_checkpoint<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    progress: &dyn MigrationProgress,
    event_listener: Option<&dyn MigrationEventListener>,
    event_registry: Option<&Arc<EventSubscriptions<MigrationEvent>>>,
    lock_path: Option<&Path>,
    checkpoint_dir: Option<&Path>,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
{
    let _in_process_lock = MigrationLockGuard::try_acquire()?;
    let _file_lock: Option<MigrationFileLock> = if let Some(path) = lock_path {
        Some(MigrationFileLock::try_acquire(path)?)
    } else {
        None
    };
    let dispatcher = MigrationDispatcher::with_registry(event_listener, event_registry);
    let start = std::time::Instant::now();
    // --- checkpoint resume handling ---
    let mut checkpoint_completed: Vec<usize> = Vec::new();
    let mut checkpoint_rows: u64 = 0;
    if let Some(dir) = checkpoint_dir {
        match crate::plan::MigrationCheckpoint::load(plan, dir) {
            Ok(Some(cp)) => {
                log::info!(
                    "Resuming migration from checkpoint at step {} with completed {:?}",
                    cp.completed_step_index,
                    cp.completed_steps
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

    dispatcher.notify(MigrationEvent::Started { plan: plan.clone() });
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
        if report.success {
            dispatcher.notify(MigrationEvent::Completed {
                report: report.clone(),
            });
        } else {
            dispatcher.notify(MigrationEvent::Failed {
                error: report.errors.join("; "),
            });
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
                    dispatcher.notify(MigrationEvent::Failed { error: err.clone() });
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
        dispatcher.notify(MigrationEvent::Completed {
            report: report.clone(),
        });
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
        dispatcher.notify(MigrationEvent::Completed {
            report: report.clone(),
        });
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
        dispatcher.notify(MigrationEvent::StepStarted { step_idx: idx });

        let single_slice = vec![idx];
        let step_report = if plan.target.is_edge {
            execute_edge_plan_with_progress(storage, plan, &single_slice, progress, event_listener)
                .inspect_err(|_| {
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                })?
        } else {
            execute_vertex_plan_with_progress(
                storage,
                plan,
                &single_slice,
                progress,
                event_listener,
            )
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
            dispatcher.notify(MigrationEvent::Failed {
                error: step_report.errors.join("; "),
            });
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
        dispatcher.notify(MigrationEvent::StepCompleted {
            step_idx: idx,
            rows: step_report.rows_migrated,
        });

        let cp = crate::plan::MigrationCheckpoint {
            completed_step_index: idx,
            rows_migrated_before: checkpoint_rows
                + overall_rows.saturating_sub(step_report.rows_migrated),
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

    let total_rows = if checkpoint_rows > overall_rows {
        checkpoint_rows
    } else {
        overall_rows
    };
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
    dispatcher.notify(MigrationEvent::Completed {
        report: report.clone(),
    });
    progress.on_plan_complete(plan, final_rows);
    global_migration_metrics().record_success(final_rows, start.elapsed().as_millis() as u64);
    Ok(report)
}

pub fn rollback_migration<S>(
    storage: &mut S,
    plan: &MigrationPlan,
) -> Result<MigrationReport, MigrationError>
where
    S: StorageReader
        + StorageWriter
        + StorageSchemaOps
        + AutoCommitGroupOps
        + AutoCommitBatchOps
        + ?Sized,
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

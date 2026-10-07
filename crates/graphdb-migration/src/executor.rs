//! Migration plan entry points: locking, checkpoint resume orchestration,
//! history recording, and rollback.
//!
//! Step-level execution lives in [`plan_runner`]; row-level step application
//! lives in [`data_apply`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use graphdb_core::error::storage::StorageErrorKind;
use graphdb_core::event_dispatch::EventSubscriptions;
use graphdb_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, MigrationHistoryManager, MigrationHistoryRecord,
    MigrationStatus, StorageReader, StorageSchemaOps, StorageWriter,
};

use crate::config::MigrationConfig;
use crate::error::MigrationError;
use crate::event::MigrationEvent;
use crate::lock::MigrationFileLock;
use crate::metrics::global_migration_metrics;
use crate::plan::{irreversible_guidance, MigrationPlan, MigrationReport, SafetyLevel};
use crate::progress::{MigrationProgress, NoopProgress};

mod data_apply;
mod plan_runner;

use plan_runner::{
    execute_dry_run, execute_edge_plan_with_progress, execute_schema_steps,
    execute_vertex_plan_with_progress, record_migration_history,
};

static MIGRATION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Write fence held across schema-modifying steps of a migration.
///
/// The migration engine never touches transaction internals directly: the
/// layer that owns the write gate (embedded API, server) supplies an
/// implementation backed by its own gate, typically a checkpoint drain fence
/// built from `graphdb_transaction::issue_request_drain`. This keeps the
/// dependency direction intact while making the online protocol reachable.
pub trait SchemaWriteFence: Send + Sync {
    /// Pause new writes and drain in-flight ones before schema steps run.
    /// A timeout error aborts the migration instead of stretching the stall.
    fn hold(&self) -> Result<(), MigrationError>;
    /// Resume writes after schema steps complete or fail. Always called.
    fn release(&self);
}

struct SchemaFenceHold<'a> {
    fence: &'a dyn SchemaWriteFence,
}

impl Drop for SchemaFenceHold<'_> {
    fn drop(&mut self) {
        self.fence.release();
    }
}

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

/// Optional collaborators for [`execute_migration_plan_with_options`].
///
/// `config` supplies batch size overrides plus default lock and checkpoint
/// paths; explicit `lock_path` / `checkpoint_dir` fields take precedence over
/// the ones embedded in `config`.
#[derive(Default)]
pub struct ExecuteOptions<'a> {
    pub config: Option<&'a MigrationConfig>,
    pub progress: Option<&'a dyn MigrationProgress>,
    pub event_registry: Option<&'a Arc<EventSubscriptions<MigrationEvent>>>,
    pub lock_path: Option<&'a Path>,
    pub checkpoint_dir: Option<&'a Path>,
    /// Optional write fence held only across schema-modifying steps so the
    /// long data backfill phase stays online while the switch window is
    /// bounded. Ignored for dry runs, which never touch storage.
    pub schema_fence: Option<&'a dyn SchemaWriteFence>,
    /// Directory for pre-migration backups of destructive steps. When set
    /// and the plan drops data, rows are snapshotted before execution so
    /// rollback can restore them.
    pub backup_dir: Option<&'a Path>,
    /// Skip the linear version-chain check. Set for rollback plans, which
    /// intentionally move against the applied history.
    pub skip_chain_check: bool,
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
    execute_migration_plan_with_options(storage, plan, ExecuteOptions::default())
}

pub fn execute_migration_plan_with_options<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    options: ExecuteOptions<'_>,
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
    let mut lock_path = options.lock_path;
    let mut checkpoint_dir = options.checkpoint_dir;
    let mut backup_dir = options.backup_dir;
    if let Some(config) = options.config {
        if config.batch_size != 0 {
            effective_plan.batch_size = config.batch_size;
        }
        lock_path = lock_path.or(config.lock_path.as_deref());
        checkpoint_dir = checkpoint_dir.or(config.checkpoint_dir.as_deref());
        backup_dir = backup_dir.or(config.backup_dir.as_deref());
    }

    if let Some(dir) = checkpoint_dir {
        let min_free = options.config.map_or(0, |c| c.min_free_bytes);
        preflight_check(dir, min_free)?;
    }

    let progress = options.progress.unwrap_or(&NoopProgress);
    let registry = options.event_registry;

    let _in_process_lock = MigrationLockGuard::try_acquire()?;
    let _file_lock: Option<MigrationFileLock> = if let Some(path) = lock_path {
        Some(MigrationFileLock::try_acquire(path)?)
    } else {
        None
    };
    let notify = |event: &MigrationEvent| {
        if let Some(registry) = registry {
            registry.dispatch("migration", event);
        }
    };
    let start = std::time::Instant::now();
    // --- checkpoint resume handling ---
    let mut checkpoint_completed: Vec<usize> = Vec::new();
    let mut checkpoint_rows: u64 = 0;
    if let Some(dir) = checkpoint_dir {
        match crate::plan::MigrationCheckpoint::load(&effective_plan, dir) {
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

    notify(&MigrationEvent::Started {
        plan: effective_plan.clone(),
    });
    progress.on_plan_start(&effective_plan);

    if let Some(guidance) = irreversible_guidance(&effective_plan) {
        log::warn!("Migration plan contains irreversible steps: {guidance}");
    }

    if effective_plan.dry_run {
        let report = execute_dry_run(storage, &effective_plan)?;
        if report.success {
            notify(&MigrationEvent::Completed {
                report: report.clone(),
            });
        } else {
            notify(&MigrationEvent::Failed {
                error: report.errors.join("; "),
            });
        }
        progress.on_plan_complete(&effective_plan, report.rows_migrated);
        return Ok(report);
    }

    if !effective_plan.plan_hash.is_empty() {
        if let Ok(existing) = storage.list_migration_history(
            &effective_plan.target.space,
            &effective_plan.target.label,
            effective_plan.target.is_edge,
        ) {
            for rec in existing {
                if rec.to_version == effective_plan.version_range.to
                    && rec.plan_hash != effective_plan.plan_hash
                {
                    let err = format!(
                        "Checksum mismatch for version {}: stored hash {} != plan hash {}",
                        rec.to_version, rec.plan_hash, effective_plan.plan_hash
                    );
                    notify(&MigrationEvent::Failed { error: err.clone() });
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                    return Err(MigrationError::Plan(err));
                }
            }
        }
    }

    let effective_remaining: Vec<usize> = (0..effective_plan.steps.len())
        .filter(|i| {
            !effective_plan.completed_steps.contains(i) && !checkpoint_completed.contains(i)
        })
        .collect();

    if effective_remaining.is_empty() {
        let mut all_done = effective_plan.completed_steps.clone();
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
        notify(&MigrationEvent::Completed {
            report: report.clone(),
        });
        progress.on_plan_complete(&effective_plan, 0);
        if let Some(dir) = checkpoint_dir {
            let _ = crate::plan::MigrationCheckpoint::cleanup(&effective_plan, dir);
        }
        global_migration_metrics()
            .record_success(report.rows_migrated, start.elapsed().as_millis() as u64);
        return Ok(report);
    }

    // Enforce the linear version chain: a plan may only start where the
    // applied history left off, and may not re-apply a target version.
    // Backends without history support fail open with a warning.
    // Rollback plans move against history and opt out via skip_chain_check.
    if !options.skip_chain_check {
        match storage.get_applied_versions(
            &effective_plan.target.space,
            &effective_plan.target.label,
            effective_plan.target.is_edge,
        ) {
            Ok(applied) => {
                if let Err(e) = MigrationHistoryManager::check_chain(
                    &applied,
                    &effective_plan.target.space,
                    &effective_plan.target.label,
                    effective_plan.target.is_edge,
                    effective_plan.version_range.from,
                    effective_plan.version_range.to,
                ) {
                    let err = format!("version chain rejected: {e}");
                    notify(&MigrationEvent::Failed { error: err.clone() });
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                    return Err(MigrationError::Plan(err));
                }
            }
            Err(e) if e.kind() == StorageErrorKind::NotSupported => {
                log::warn!("Migration history not supported; skipping version chain check");
            }
            Err(e) => {
                let err = format!("cannot verify migration chain: {e}");
                notify(&MigrationEvent::Failed { error: err.clone() });
                global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                return Err(MigrationError::Plan(err));
            }
        }
    }

    // Snapshot rows before destructive steps run. Without a backup directory
    // the plan still executes, but rollback of drops stays unavailable.
    if effective_plan.requires_backup() {
        match backup_dir {
            Some(dir) => {
                match crate::backup::write_backup(&*storage, &effective_plan, dir) {
                    Ok(rows) => log::info!(
                        "Backed up {rows} row(s) for destructive migration {}/{}",
                        effective_plan.target.space,
                        effective_plan.target.label
                    ),
                    Err(e) => {
                        let err = format!("pre-migration backup failed: {e}");
                        notify(&MigrationEvent::Failed { error: err.clone() });
                        global_migration_metrics()
                            .record_failure(start.elapsed().as_millis() as u64);
                        return Err(MigrationError::Plan(err));
                    }
                }
            }
            None => log::warn!(
                "Destructive migration plan for {}/{} has no backup_dir; drops cannot be rolled back",
                effective_plan.target.space,
                effective_plan.target.label
            ),
        }
    }

    // Handle schema-modifying steps first. When the caller supplied a write
    // fence, hold it only across this bounded switch window; the guard drops
    // at the end of the block so the data backfill below stays online.
    let schema_executed = {
        let needs_fence = effective_remaining
            .iter()
            .any(|i| effective_plan.steps[*i].is_schema_modifying());
        let _fence_hold: Option<SchemaFenceHold<'_>> = if needs_fence {
            match options.schema_fence {
                Some(fence) => {
                    fence.hold()?;
                    Some(SchemaFenceHold { fence })
                }
                None => None,
            }
        } else {
            None
        };
        execute_schema_steps(storage, &effective_plan, &effective_remaining, progress).inspect_err(
            |_| {
                global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
            },
        )?
    };
    let data_remaining: Vec<usize> = effective_remaining
        .into_iter()
        .filter(|idx| {
            !schema_executed.contains(idx) && !effective_plan.steps[*idx].is_schema_modifying()
        })
        .collect();

    if data_remaining.is_empty() {
        let mut all_completed = effective_plan.completed_steps.clone();
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
                let _ = cp.save(&effective_plan, dir);
            }
            let _ = crate::plan::MigrationCheckpoint::cleanup(&effective_plan, dir);
        }
        let report = MigrationReport {
            success: true,
            steps_completed: all_completed.len(),
            rows_migrated: 0,
            errors: vec![],
            completed_step_indices: all_completed.clone(),
        };
        record_migration_history(storage, &effective_plan, 0, MigrationStatus::Applied, None);
        notify(&MigrationEvent::Completed {
            report: report.clone(),
        });
        progress.on_plan_complete(&effective_plan, 0);
        global_migration_metrics()
            .record_success(report.rows_migrated, start.elapsed().as_millis() as u64);
        return Ok(report);
    }

    // Prepare combined completed set including schema
    let mut all_completed: Vec<usize> = {
        let mut v = effective_plan.completed_steps.clone();
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
            if let Err(e) = cp.save(&effective_plan, dir) {
                log::warn!("Failed to save checkpoint after schema steps: {}", e);
            }
        }
    }

    let mut overall_rows: u64 = 0;
    // Per-step loop with checkpoint save after each step
    for &idx in &data_remaining {
        let step = &effective_plan.steps[idx];
        progress.on_step_start(idx, step);
        notify(&MigrationEvent::StepStarted { step_idx: idx });

        let single_slice = vec![idx];
        let step_report = if effective_plan.target.is_edge {
            execute_edge_plan_with_progress(storage, &effective_plan, &single_slice, progress)
                .inspect_err(|_| {
                    global_migration_metrics().record_failure(start.elapsed().as_millis() as u64);
                })?
        } else {
            execute_vertex_plan_with_progress(storage, &effective_plan, &single_slice, progress)
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
                let _ = cp.save(&effective_plan, dir);
            }
            record_migration_history(
                storage,
                &effective_plan,
                0,
                MigrationStatus::Failed,
                Some(step_report.errors.join("; ")),
            );
            notify(&MigrationEvent::Failed {
                error: step_report.errors.join("; "),
            });
            for err in &step_report.errors {
                progress.on_error(err);
            }
            progress.on_plan_complete(&effective_plan, 0);
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
        notify(&MigrationEvent::StepCompleted {
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
            if let Err(e) = cp.save(&effective_plan, dir) {
                log::warn!("Failed to save checkpoint for step {}: {}", idx, e);
            }
        }
    }

    // All data steps succeeded
    if let Some(dir) = checkpoint_dir {
        let _ = crate::plan::MigrationCheckpoint::cleanup(&effective_plan, dir);
    }

    // Keep max: resumed checkpoint rows already represent the previous
    // total and overall_rows counts the remaining steps over the same set.
    let final_rows = checkpoint_rows.max(overall_rows);
    let report = MigrationReport {
        success: true,
        steps_completed: all_completed.len(),
        rows_migrated: final_rows,
        errors: vec![],
        completed_step_indices: all_completed.clone(),
    };
    record_migration_history(
        storage,
        &effective_plan,
        final_rows,
        MigrationStatus::Applied,
        None,
    );
    notify(&MigrationEvent::Completed {
        report: report.clone(),
    });
    progress.on_plan_complete(&effective_plan, final_rows);
    global_migration_metrics().record_success(final_rows, start.elapsed().as_millis() as u64);
    Ok(report)
}

fn no_rollback_error(plan: &MigrationPlan) -> MigrationError {
    let guidance = irreversible_guidance(plan);
    match plan.overall_safety {
        SafetyLevel::Dangerous => {
            let hint = guidance.unwrap_or_default();
            MigrationError::Plan(format!(
                "Cannot rollback a dangerous migration (data loss). {hint}"
            ))
        }
        _ => MigrationError::Plan("No rollback plan available".to_string()),
    }
}

/// Verify the checkpoint directory is writable and, when `min_free_bytes` is
/// set, that its filesystem has enough space for checkpoint files.
fn preflight_check(checkpoint_dir: &Path, min_free_bytes: u64) -> Result<(), MigrationError> {
    std::fs::create_dir_all(checkpoint_dir).map_err(|e| {
        MigrationError::Preflight(format!(
            "cannot create checkpoint directory {}: {e}",
            checkpoint_dir.display()
        ))
    })?;
    if min_free_bytes > 0 {
        let free = fs4::available_space(checkpoint_dir)
            .map_err(|e| MigrationError::Preflight(format!("cannot read free space: {e}")))?;
        if free < min_free_bytes {
            return Err(MigrationError::Preflight(format!(
                "insufficient disk space at {}: need {} bytes, {} available",
                checkpoint_dir.display(),
                min_free_bytes,
                free
            )));
        }
    }
    Ok(())
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
    rollback_migration_with_options(storage, plan, None)
}

/// Rollback with an optional backup directory. Plans without a generated
/// rollback plan (typically destructive ones) are restored from the
/// pre-migration backup when one exists.
pub fn rollback_migration_with_options<S>(
    storage: &mut S,
    plan: &MigrationPlan,
    backup_dir: Option<&Path>,
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
        Some(rollback) => execute_migration_plan_with_options(
            storage,
            rollback,
            ExecuteOptions {
                skip_chain_check: true,
                ..Default::default()
            },
        ),
        None => {
            if let Some(dir) = backup_dir {
                match crate::backup::restore_backup(storage, plan, dir)? {
                    Some(report) => Ok(report),
                    None => Err(no_rollback_error(plan)),
                }
            } else {
                Err(no_rollback_error(plan))
            }
        }
    };
    if let Ok(ref report) = result {
        if report.success {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
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
                applied_at: now,
                completed_at: Some(now),
                error_message: None,
            };
            let _ = storage.record_migration_history(rollback_record);
        }
    }
    result
}

#[cfg(test)]
mod tests;

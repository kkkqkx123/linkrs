use crate::plan::{MigrationPlan, MigrationStep};

pub trait MigrationProgress: Send + Sync {
    fn on_plan_start(&self, _plan: &MigrationPlan) {}
    fn on_step_start(&self, _step_idx: usize, _step: &MigrationStep) {}
    fn on_step_complete(&self, _step_idx: usize, _step: &MigrationStep) {}
    fn on_row_processed(&self, _rows: u64) {}
    fn on_plan_complete(&self, _plan: &MigrationPlan, _rows_migrated: u64) {}
    fn on_error(&self, _error: &str) {}
}

#[derive(Debug, Clone, Copy)]
pub struct NoopProgress;

impl MigrationProgress for NoopProgress {}

use crate::plan::{MigrationPlan, MigrationReport};

#[derive(Debug, Clone)]
pub enum MigrationEvent {
    Started { plan: MigrationPlan },
    StepStarted { step_idx: usize },
    StepCompleted { step_idx: usize, rows: u64 },
    Completed { report: MigrationReport },
    Failed { error: String },
    RolledBack { report: MigrationReport },
}

pub trait MigrationEventListener: Send + Sync {
    fn on_event(&self, event: MigrationEvent);
}

#[derive(Debug, Clone, Copy)]
pub struct NoopEventListener;

impl MigrationEventListener for NoopEventListener {
    fn on_event(&self, _event: MigrationEvent) {}
}

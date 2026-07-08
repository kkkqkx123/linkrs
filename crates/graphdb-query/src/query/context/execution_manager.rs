//! Query Execution Manager
//!
//! Manage the execution plan and termination signals during the execution of queries.

use crate::query::planning::plan::ExecutionPlan;
use std::sync::atomic::{AtomicBool, Ordering};

/// Query Execution Manager
///
/// Manage critical information during the execution of queries, including:
/// - Execute the plan.
/// - Was it terminated?
/// - Other management functions related to the execution of tasks.
pub struct QueryExecutionManager {
    /// Execution Plan
    plan: Option<Box<ExecutionPlan>>,

    /// Has it been marked as terminated?
    killed: AtomicBool,
}

impl QueryExecutionManager {
    /// Create a new Execution Manager.
    pub fn new() -> Self {
        Self {
            plan: None,
            killed: AtomicBool::new(false),
        }
    }

    /// Obtain the execution plan
    pub fn plan(&self) -> Option<ExecutionPlan> {
        self.plan.as_ref().map(|p| *p.clone())
    }

    /// Setting up the execution plan
    pub fn set_plan(&mut self, plan: ExecutionPlan) {
        self.plan = Some(Box::new(plan));
    }

    /// Obtain the execution plan ID
    pub fn plan_id(&self) -> Option<i64> {
        self.plan.as_ref().map(|p| p.id)
    }

    /// Marked as terminated
    pub fn mark_killed(&self) {
        self.killed.store(true, Ordering::SeqCst);
        log::info!("Query execution manager marked as killed");
    }

    /// Check whether it was terminated.
    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    /// Reset the Execution Manager
    pub fn reset(&mut self) {
        self.plan = None;
        self.killed.store(false, Ordering::SeqCst);
        log::info!("Query execution manager reset");
    }
}

impl Default for QueryExecutionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for QueryExecutionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryExecutionManager")
            .field("plan_id", &self.plan_id())
            .field("killed", &self.killed)
            .finish()
    }
}

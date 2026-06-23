//! Implementation of basic actuators
//!
//! Provide the basic structure and common functions of an executor, including the Executor trait, the HasStorage trait, the HasInput trait, etc.

use std::sync::Arc;
use std::time::Instant;

use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;
use parking_lot::RwLock;

use super::execution_context::ExecutionContext;
use super::execution_result::{DBResult, ExecutionResult};
use super::executor_stats::ExecutorStats;
use crate::query::executor::base::ExecutorEnum;

/// A unified Executor trait
///
/// The core trait that all actuators must implement includes functions for execution, lifecycle management, and metadata handling.
pub trait Executor<S>: Send {
    /// Please provide the text you would like to have translated.
    fn execute(&mut self) -> DBResult<ExecutionResult>;

    /// Activate the actuator.
    fn open(&mut self) -> DBResult<()>;

    /// Turn off the actuator.
    fn close(&mut self) -> DBResult<()>;

    /// Check whether the actuator has been turned on.
    fn is_open(&self) -> bool;

    /// Obtain the executor ID
    fn id(&self) -> i64;

    /// Obtain the name of the executor.
    fn name(&self) -> &str;

    /// Obtain the executor description.
    fn description(&self) -> &str;

    /// Obtain execution statistics information.
    fn stats(&self) -> &ExecutorStats;

    /// Obtain variable execution statistics information.
    fn stats_mut(&mut self) -> &mut ExecutorStats;

    /// Check the memory usage.
    fn check_memory(&self) -> DBResult<()> {
        Ok(())
    }
}

/// "Storage Access Trait"
///
/// Only executors that have access to storage capabilities can implement this trait.
pub trait HasStorage<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>>;
}

/// “Input Access Trait” – A unified mechanism for handling user input
///
/// Executors that need to access the input data should implement this trait.
pub trait HasInput<S> {
    fn get_input(&self) -> Option<&ExecutionResult>;
    fn set_input(&mut self, input: ExecutionResult);
}

/// Input Executor trait
///
/// Used to process input data from other actuators.
/// Replace `Box<dyn Executor<S>>` with `ExecutorEnum` to achieve static distribution.
pub trait InputExecutor<S: StorageClient + Send + 'static> {
    fn set_input(&mut self, input: ExecutorEnum<S>);
    fn get_input(&self) -> Option<&ExecutorEnum<S>>;
}

/// An executor trait that can be executed in a chained manner
///
/// Executors that support chained combination can implement this trait.
pub trait ChainableExecutor<S: StorageClient + Send + 'static>:
    Executor<S> + InputExecutor<S>
{
}

/// Basic Executor
///
/// Provide general functions for actuators, including storage access, statistical information, lifecycle management, etc.
#[derive(Clone, Debug)]
pub struct BaseExecutor<S> {
    /// Actuator ID
    pub id: i64,
    /// Actuator name
    pub name: String,
    /// Actuator description
    pub description: String,
    /// Storage engine reference
    pub storage: Option<Arc<RwLock<S>>>,
    /// Of course! Please provide the text you would like to have translated.
    pub context: ExecutionContext,
    /// Has it been turned on?
    is_open: bool,
    /// Generate statistical information.
    stats: ExecutorStats,
}

impl<S> BaseExecutor<S> {
    /// Create a new basic executor (with storage).
    pub fn new(
        id: i64,
        name: String,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: Some(storage),
            context: ExecutionContext::new(expr_context),
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }

    /// Create a new basic executor (without storage).
    pub fn without_storage(
        id: i64,
        name: String,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: None,
            context: ExecutionContext::new(expr_context),
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }

    /// Create a basic executor with context
    pub fn with_context(
        id: i64,
        name: String,
        storage: Arc<RwLock<S>>,
        context: ExecutionContext,
    ) -> Self {
        Self {
            id,
            name,
            description: String::new(),
            storage: Some(storage),
            context,
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }

    /// Create a basic executor with a description
    pub fn with_description(
        id: i64,
        name: String,
        description: String,
        storage: Arc<RwLock<S>>,
    ) -> Self {
        Self {
            id,
            name,
            description,
            storage: Some(storage),
            context: ExecutionContext::default(),
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }

    /// Create a basic executor with context and description
    pub fn with_context_and_description(
        id: i64,
        name: String,
        description: String,
        storage: Arc<RwLock<S>>,
        context: ExecutionContext,
    ) -> Self {
        Self {
            id,
            name,
            description,
            storage: Some(storage),
            context,
            is_open: false,
            stats: ExecutorStats::new(),
        }
    }

    /// Obtain execution statistics (immutable reference)
    pub fn get_stats(&self) -> &ExecutorStats {
        &self.stats
    }

    /// Obtain execution statistics information (variable reference)
    pub fn get_stats_mut(&mut self) -> &mut ExecutorStats {
        &mut self.stats
    }
}

impl<S> HasStorage<S> for BaseExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.storage.as_ref().expect("Storage not set")
    }
}

impl<S: Send + Sync> Executor<S> for BaseExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = Ok(ExecutionResult::Success);
        self.stats_mut().add_total_time(start.elapsed());
        result
    }

    fn open(&mut self) -> DBResult<()> {
        self.is_open = true;
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        self.is_open = false;
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.is_open
    }

    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn stats(&self) -> &ExecutorStats {
        &self.stats
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        &mut self.stats
    }
}

/// Start the executor
///
/// It indicates the starting point for the execution of the query; no actual data is generated.
#[derive(Debug)]
pub struct StartExecutor<S> {
    base: BaseExecutor<S>,
}

impl<S> StartExecutor<S> {
    /// Create a new start executor.
    pub fn new(id: i64, expr_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self {
            base: BaseExecutor::without_storage(id, "StartExecutor".to_string(), expr_context),
        }
    }
}

impl<S: Send + Sync> Executor<S> for StartExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let start = Instant::now();
        let result = Ok(ExecutionResult::Success);
        self.base.get_stats_mut().add_total_time(start.elapsed());
        result
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        &self.base.description
    }

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

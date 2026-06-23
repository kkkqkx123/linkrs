//! Control Flow Executor Builder
//!
//! Responsible for creating executors of different control flow types (Loop, Select, Argument, PassThrough, DataCollect).

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::control_flow::{LoopExecutor, SelectExecutor};
use crate::query::executor::utils::{ArgumentExecutor, DataCollectExecutor, PassThroughExecutor};
use crate::query::planning::plan::control_flow::{
    ArgumentNode, BeginTransactionNode, CommitNode, LoopNode, PassThroughNode, RollbackNode,
    SelectNode,
};
use crate::query::planning::plan::DataCollectNode;
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Create executor function type alias
type CreateExecutorFn<S> = dyn FnMut(
    &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    Arc<RwLock<S>>,
    &ExecutionContext,
) -> Result<ExecutorEnum<S>, QueryError>;

/// Control Flow Executor Builder
pub struct ControlFlowBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> ControlFlowBuilder<S> {
    /// Create a new control flow builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Constructing a Loop Executor
    pub fn build_loop(
        node: &LoopNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        create_executor_fn: &mut CreateExecutorFn<S>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let body = node
            .body()
            .as_ref()
            .ok_or_else(|| QueryError::execution("Loop node missing body".to_string()))?;

        let body_executor = create_executor_fn(body, storage.clone(), context)?;

        let condition = node
            .condition()
            .expression()
            .map(|meta| meta.inner().clone());

        let executor = LoopExecutor::new(
            node.id(),
            storage,
            condition,
            body_executor,
            None,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Loop(executor))
    }

    /// Building the Select Executor
    pub fn build_select(
        node: &SelectNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        create_executor_fn: &mut CreateExecutorFn<S>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let condition = node
            .condition()
            .expression()
            .map(|meta| meta.inner().clone())
            .unwrap_or_else(|| crate::core::Expression::Literal(crate::core::Value::Bool(true)));

        let if_branch = node
            .if_branch()
            .as_ref()
            .ok_or_else(|| QueryError::execution("Select node missing if_branch".to_string()))?;

        let if_executor = create_executor_fn(if_branch, storage.clone(), context)?;

        let else_executor = node
            .else_branch()
            .as_ref()
            .map(|branch| create_executor_fn(branch, storage.clone(), context))
            .transpose()?;

        let executor = SelectExecutor::new(
            node.id(),
            storage,
            condition,
            if_executor,
            else_executor,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Select(executor))
    }

    /// Constructing an Argument Executor
    pub fn build_argument(
        node: &ArgumentNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor = ArgumentExecutor::new(
            node.id(),
            storage,
            node.var(),
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Argument(executor))
    }

    /// Constructing a PassThrough executor
    pub fn build_pass_through(
        node: &PassThroughNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    /// Building the DataCollect executor
    pub fn build_data_collect(
        node: &DataCollectNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            DataCollectExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::DataCollect(executor))
    }

    /// Building the BeginTransaction executor
    pub fn build_begin_transaction(
        node: &BeginTransactionNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    /// Building the Commit executor
    pub fn build_commit(
        node: &CommitNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }

    /// Building the Rollback executor
    pub fn build_rollback(
        node: &RollbackNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let executor =
            PassThroughExecutor::new(node.id(), storage, context.expression_context().clone());
        Ok(ExecutorEnum::PassThrough(executor))
    }
}

impl<S: StorageClient + 'static> Default for ControlFlowBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

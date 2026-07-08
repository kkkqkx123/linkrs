//! Plan Executor Engine

use crate::core::error::query::QueryError;
use crate::query::executor::base::{ExecutionContext, ExecutionResult, Executor, InputExecutor};
use crate::query::executor::factory::ExecutorFactory;
use crate::query::executor::utils::object_pool::ThreadSafeExecutorPool;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::ExecutionPlan;
use crate::query::planning::plan::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Find the input_var from an ExpandAllNode in the plan tree
fn find_expand_all_input_var(node: &PlanNodeEnum) -> Option<String> {
    if let Some(expand_all) = node.as_expand_all() {
        expand_all.get_input_var().map(|v| v.to_string())
    } else {
        for child in node.children() {
            if let Some(var) = find_expand_all_input_var(child) {
                return Some(var);
            }
        }
        None
    }
}

fn find_expand_all_dst_var(node: &PlanNodeEnum) -> Option<String> {
    if let Some(expand_all) = node.as_expand_all() {
        let col_names = expand_all.col_names();
        if col_names.len() > 2 {
            Some(col_names[2].clone())
        } else {
            None
        }
    } else {
        None
    }
}

/// Plan Executor
pub struct PlanExecutor<S: StorageClient + Send + 'static> {
    factory: ExecutorFactory<S>,
    object_pool: Option<Arc<ThreadSafeExecutorPool<S>>>,
}

impl<S: StorageClient + Send + 'static> PlanExecutor<S> {
    /// Create a new plan executor.
    pub fn new(factory: ExecutorFactory<S>) -> Self {
        Self {
            factory,
            object_pool: None,
        }
    }

    /// Create a new plan executor with object pool.
    pub fn with_object_pool(
        factory: ExecutorFactory<S>,
        object_pool: Arc<ThreadSafeExecutorPool<S>>,
    ) -> Self {
        Self {
            factory,
            object_pool: Some(object_pool),
        }
    }

    /// Recursively build an executor chain from a plan tree node.
    ///
    /// For `SingleInputNode` plan nodes (e.g. Project, Filter), this creates the executor
    /// for the node itself, then recursively builds its child executor and connects it
    /// via `set_input`. For `BinaryInputNode` plan nodes (e.g. Join), both children are built
    /// and executed to store their results in the execution context.
    /// For `ZeroInputNode` plan nodes (leaf nodes), only the executor itself is created.
    fn build_executor_chain(
        &mut self,
        plan_node: &crate::query::planning::plan::PlanNodeEnum,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<crate::query::executor::ExecutorEnum<S>, QueryError> {
        let mut executor = self
            .factory
            .create_executor(plan_node, storage.clone(), context)?;

        let children = plan_node.children();

        match children.len() {
            0 => {
                // ZeroInputNode: no child nodes to process
            }
            1 => {
                // SingleInputNode: build child and set as input
                let child_executor =
                    self.build_executor_chain(children[0], storage.clone(), context)?;
                executor.set_input(child_executor);
            }
            2 => {
                // BinaryInputNode (e.g., Join): build and execute both children
                let mut left_executor =
                    self.build_executor_chain(children[0], storage.clone(), context)?;
                let left_result = left_executor.execute().map_err(|e| {
                    QueryError::execution(format!("Left child execution failed: {}", e))
                })?;

                // Get left variable name from left child's output_var
                // This must match the variable name used by the join executor (from extract_join_vars)
                let left_var = children[0]
                    .output_var()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("left_{}", plan_node.id()));
                context.set_result(left_var.clone(), left_result.clone());

                // If right child (or its descendants) is ExpandAllNode with input_var,
                // also store the result under that variable name
                // This allows ExpandAllExecutor to find the input using its input_var
                if let Some(input_var) = find_expand_all_input_var(children[1]) {
                    if input_var != left_var {
                        context.set_result(input_var, left_result);
                    }
                }

                let mut right_executor =
                    self.build_executor_chain(children[1], storage.clone(), context)?;
                let right_result = right_executor.execute().map_err(|e| {
                    QueryError::execution(format!("Right child execution failed: {}", e))
                })?;

                // Get right variable name from node's output_var or use default
                let right_var = children[1]
                    .output_var()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("right_{}", plan_node.id()));
                context.set_result(right_var, right_result);
            }
            _ => {
                for (i, child) in children.iter().enumerate() {
                    let mut child_executor =
                        self.build_executor_chain(child, storage.clone(), context)?;
                    let child_result = child_executor.execute().map_err(|e| {
                        QueryError::execution(format!("Child {} execution failed: {}", i, e))
                    })?;

                    let child_var = child
                        .output_var()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| format!("child_{}_{}", plan_node.id(), i));
                    context.set_result(child_var, child_result.clone());

                    if let Some(dst_var) = find_expand_all_dst_var(child) {
                        context.set_result(dst_var, child_result);
                    }
                }

                if let Some(input_var) = find_expand_all_input_var(plan_node) {
                    if let Some(first_child) = children.first() {
                        let first_var = first_child
                            .output_var()
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| format!("child_{}_0", plan_node.id()));
                        if input_var != first_var {
                            if let Some(first_result) = context.get_result(&first_var) {
                                context.set_result(input_var, first_result);
                            }
                        }
                    }
                }
            }
        }

        Ok(executor)
    }

    /// Execute the query plan and return the result.
    pub fn execute_plan(
        &mut self,
        plan: &ExecutionPlan,
        storage: Arc<RwLock<S>>,
        expression_context: Arc<ExpressionAnalysisContext>,
    ) -> Result<ExecutionResult, QueryError> {
        let context = ExecutionContext::new(expression_context);

        // Build the executor chain from the plan root
        let root_node = plan
            .root()
            .as_ref()
            .ok_or_else(|| QueryError::execution("Execution plan has no root node".to_string()))?;
        let mut root_executor = self.build_executor_chain(root_node, storage.clone(), &context)?;

        // Execute the root executor
        root_executor
            .execute()
            .map_err(|e| QueryError::execution(e.to_string()))
    }

    /// Get the executor factory.
    pub fn factory(&self) -> &ExecutorFactory<S> {
        &self.factory
    }

    /// Get the mutable executor factory.
    pub fn factory_mut(&mut self) -> &mut ExecutorFactory<S> {
        &mut self.factory
    }

    /// Set the object pool.
    pub fn set_object_pool(&mut self, pool: Arc<ThreadSafeExecutorPool<S>>) {
        self.object_pool = Some(pool);
    }

    /// Get the object pool.
    pub fn object_pool(&self) -> Option<&Arc<ThreadSafeExecutorPool<S>>> {
        self.object_pool.as_ref()
    }
}

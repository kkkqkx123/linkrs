//! Collection Operation Executor Builder
//!
//! Responsible for creating executors for set operation types (Union, Minus, Intersect)

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::relational_algebra::set_operations::{
    IntersectExecutor, MinusExecutor, UnionExecutor,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use crate::query::planning::plan::core::nodes::{IntersectNode, MinusNode, UnionNode};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Set Operation Executor Builder
pub struct SetOperationBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> SetOperationBuilder<S> {
    /// Create a new collection operation builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Building a Union executor
    pub fn build_union(
        node: &UnionNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // The UnionExecutor requires left_input_var and right_input_var.
        // The left var must match how the engine stores the left child's result:
        // children[0].output_var() or left_{plan_node.id()}
        let left_var = node
            .input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_var = node
            .union_input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let executor = UnionExecutor::with_context(
            node.id(),
            storage,
            left_var,
            right_var,
            node.distinct(),
            context.clone(),
        );
        Ok(ExecutorEnum::Union(executor))
    }

    /// Building the Minus executor
    pub fn build_minus(
        node: &MinusNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // The `MinusExecutor` requires `left_input_var` and `right_input_var`.
        // The left var must match how the engine stores the left child's result:
        // children[0].output_var() or left_{plan_node.id()}
        let left_var = node
            .input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_var = node
            .minus_input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let executor =
            MinusExecutor::with_context(node.id(), storage, left_var, right_var, context.clone());
        Ok(ExecutorEnum::Minus(executor))
    }

    /// Constructing the Intersect executor
    pub fn build_intersect(
        node: &IntersectNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // The IntersectExecutor requires the left_input_var and right_input_var parameters.
        // The left var must match how the engine stores the left child's result:
        // children[0].output_var() or left_{plan_node.id()}
        let left_var = node
            .input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_var = node
            .intersect_input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let executor = IntersectExecutor::with_context(
            node.id(),
            storage,
            left_var,
            right_var,
            context.clone(),
        );
        Ok(ExecutorEnum::Intersect(executor))
    }
}

impl<S: StorageClient + 'static> Default for SetOperationBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

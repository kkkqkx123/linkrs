//! Connection Executor Builder
//!
//! Responsible for creating executors for different types of joins (InnerJoin, LeftJoin, FullOuterJoin, CrossJoin)

use crate::core::error::query::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::relational_algebra::join::{
    CrossJoinExecutor, FullOuterJoinExecutor, HashInnerJoinExecutor, HashLeftJoinExecutor,
    InnerJoinConfig, InnerJoinExecutor, LeftJoinConfig, LeftJoinExecutor,
};
use crate::query::planning::plan::core::nodes::base::plan_node_traits::JoinNode;
use crate::query::planning::plan::core::nodes::{
    CrossJoinNode, FullOuterJoinNode, HashInnerJoinNode, HashLeftJoinNode, InnerJoinNode,
    LeftJoinNode, RightJoinNode, SemiJoinNode,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Connection Executor Builder
pub struct JoinBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> JoinBuilder<S> {
    /// Create a new connection builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Extract the variable names associated with the connection operations.
    fn extract_join_vars<N: JoinNode>(node: &N) -> (String, String) {
        // Use the `output_var` of the node as the variable name; if it is not set, a default value will be generated.
        let left_var = node
            .left_input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_var = node
            .right_input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("right_{}", node.id()));
        (left_var, right_var)
    }

    /// Constructing the InnerJoin executor
    pub fn build_inner_join(
        node: &InnerJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config = InnerJoinConfig {
            id: node.id(),
            hash_keys,
            probe_keys,
            left_var,
            right_var,
            col_names: node.col_names().to_vec(),
        };

        let executor =
            InnerJoinExecutor::new(storage, context.expression_context().clone(), config);
        Ok(ExecutorEnum::InnerJoin(executor))
    }

    /// Constructing the HashInnerJoin executor
    pub fn build_hash_inner_join(
        node: &HashInnerJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config = InnerJoinConfig {
            id: node.id(),
            hash_keys,
            probe_keys,
            left_var,
            right_var,
            col_names: node.col_names().to_vec(),
        };

        let executor = HashInnerJoinExecutor::with_context(storage, context.clone(), config);
        Ok(ExecutorEnum::HashInnerJoin(executor))
    }

    /// Building the LeftJoin executor
    pub fn build_left_join(
        node: &LeftJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config = LeftJoinConfig {
            id: node.id(),
            hash_keys,
            probe_keys,
            left_var,
            right_var,
            col_names: node.col_names().to_vec(),
        };

        let executor = LeftJoinExecutor::new(storage, context.expression_context().clone(), config);
        Ok(ExecutorEnum::LeftJoin(executor))
    }

    /// Constructing the HashLeftJoin executor
    pub fn build_hash_left_join(
        node: &HashLeftJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config = LeftJoinConfig {
            id: node.id(),
            hash_keys,
            probe_keys,
            left_var,
            right_var,
            col_names: node.col_names().to_vec(),
        };

        let executor =
            HashLeftJoinExecutor::new(storage, context.expression_context().clone(), config);
        Ok(ExecutorEnum::HashLeftJoin(executor))
    }

    /// Constructing the FullOuterJoin executor
    pub fn build_full_outer_join(
        node: &FullOuterJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config =
            crate::query::executor::relational_algebra::join::full_outer_join::FullOuterJoinConfig {
                hash_keys,
                probe_keys,
                left_var,
                right_var,
                output_columns: node.col_names().to_vec(),
            };
        let executor = FullOuterJoinExecutor::new(
            node.id(),
            storage,
            context.expression_context().clone(),
            config,
        );
        Ok(ExecutorEnum::FullOuterJoin(executor))
    }

    /// Building the CrossJoin executor
    pub fn build_cross_join(
        node: &CrossJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // The CrossJoinExecutor requires a list of input variables of type Vec<String>.
        // Check if right child is ExpandAllNode with input_var set
        let (left_var, right_var) = if let Some(expand_all) = node.right_input().as_expand_all() {
            if let Some(input_var) = expand_all.get_input_var() {
                (
                    input_var.to_string(),
                    node.right_input()
                        .output_var()
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| format!("right_{}", node.id())),
                )
            } else {
                let left_var = node
                    .left_input()
                    .output_var()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("left_{}", node.id()));
                let right_var = node
                    .right_input()
                    .output_var()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| format!("right_{}", node.id()));
                (left_var, right_var)
            }
        } else {
            let left_var = node
                .left_input()
                .output_var()
                .map(|v| v.to_string())
                .unwrap_or_else(|| format!("left_{}", node.id()));
            let right_var = node
                .right_input()
                .output_var()
                .map(|v| v.to_string())
                .unwrap_or_else(|| format!("right_{}", node.id()));
            (left_var, right_var)
        };

        let input_vars = vec![left_var, right_var];

        let executor = CrossJoinExecutor::with_context(
            node.id(),
            storage,
            input_vars,
            node.col_names().to_vec(),
            context.clone(),
        );
        Ok(ExecutorEnum::CrossJoin(executor))
    }

    /// Building the RightJoin executor
    /// RightJoin is implemented by converting to LeftJoin with swapped inputs
    pub fn build_right_join(
        node: &RightJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let left_join_node = node.to_left_join();
        Self::build_left_join(&left_join_node, storage, context)
    }

    /// Building the SemiJoin executor
    pub fn build_semi_join(
        node: &SemiJoinNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let (left_var, right_var) = Self::extract_join_vars(node);
        let hash_keys: Vec<crate::core::types::ContextualExpression> = node.hash_keys().to_vec();
        let probe_keys: Vec<crate::core::types::ContextualExpression> = node.probe_keys().to_vec();

        let config = InnerJoinConfig {
            id: node.id(),
            hash_keys,
            probe_keys,
            left_var,
            right_var,
            col_names: node.col_names().to_vec(),
        };

        let executor =
            InnerJoinExecutor::new(storage, context.expression_context().clone(), config);
        Ok(ExecutorEnum::InnerJoin(executor))
    }
}

impl<S: StorageClient + 'static> Default for JoinBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

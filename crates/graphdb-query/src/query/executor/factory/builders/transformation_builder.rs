//! Data Conversion Executor Builder
//!
//! Responsible for creating executors for various data transformation types (Unwind, Assign, Materialize, AppendVertices, RollUpApply, PatternApply).

use crate::core::error::query::QueryError;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::{
    AppendVerticesConfig, ExecutionContext, ExecutorConfig, PatternApplyConfig, RollupApplyConfig,
};
use crate::query::executor::graph_operations::MaterializeExecutor;
use crate::query::executor::result_processing::transformations::{
    AppendVerticesExecutor, AssignExecutor, PatternApplyExecutor, RollUpApplyExecutor,
    UnwindExecutor,
};
use crate::query::planning::plan::core::nodes::{
    AppendVerticesNode, ApplyNode, AssignNode, MaterializeNode, PatternApplyNode, RollUpApplyNode,
    UnwindNode,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Data Conversion Executor Builder
pub struct TransformationBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> TransformationBuilder<S> {
    /// Create a new data transformation builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Building the Unwind executor
    pub fn build_unwind(
        node: &UnwindNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

        let unwind_expression = node
            .list_expression()
            .expression()
            .map(|meta| meta.inner().clone())
            .ok_or_else(|| {
                QueryError::execution("Expression does not exist in context".to_string())
            })?;

        let input_var = node
            .input()
            .output_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| "input".to_string());

        let executor = UnwindExecutor::new(
            node.id(),
            storage,
            input_var,
            unwind_expression,
            node.col_names().to_vec(),
            true,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Unwind(executor))
    }

    /// Constructing the Assign executor
    pub fn build_assign(
        node: &AssignNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let mut parsed_assignments = Vec::new();
        for (var_name, ctx_expr) in node.assignments() {
            let expression = ctx_expr
                .expression()
                .map(|meta| meta.inner().clone())
                .ok_or_else(|| {
                    QueryError::execution("Expression does not exist in context".to_string())
                })?;
            parsed_assignments.push((var_name.clone(), expression));
        }

        let executor = AssignExecutor::new(
            node.id(),
            storage,
            parsed_assignments,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Assign(executor))
    }

    /// Building the Materialize executor
    pub fn build_materialize(
        node: &MaterializeNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Create a materialized executor, using the default memory limit (100MB).
        let executor = MaterializeExecutor::new(
            node.id(),
            storage,
            None, // Use the default memory limit.
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Materialize(executor))
    }

    /// Constructing the AppendVertices executor
    pub fn build_append_vertices(
        node: &AppendVerticesNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let input_var = node
            .input_var()
            .map(|v| v.to_string())
            .unwrap_or_else(|| format!("input_{}", node.id()));

        let src_expression = node
            .src_expression()
            .and_then(|ctx_expr| ctx_expr.expression())
            .map(|meta| meta.inner().clone())
            .unwrap_or_else(|| crate::core::Expression::Variable("_".to_string()));

        let executor = AppendVerticesExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            AppendVerticesConfig {
                input_var,
                src_expression,
                v_filter: None,
                col_names: node.col_names().to_vec(),
                dedup: node.dedup(),
                need_fetch_prop: node.need_fetch_prop(),
            },
        );
        Ok(ExecutorEnum::AppendVertices(executor))
    }

    /// Constructing the RollUpApply executor
    pub fn build_rollup_apply(
        node: &RollUpApplyNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let left_input_var = node
            .left_input_var()
            .cloned()
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_input_var = node
            .right_input_var()
            .cloned()
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let compare_cols: Vec<crate::core::Expression> = node
            .compare_cols()
            .iter()
            .map(|col| crate::core::Expression::Variable(col.clone()))
            .collect();

        let collect_col = node
            .collect_col()
            .map(|col| crate::core::Expression::Variable(col.clone()))
            .unwrap_or_else(|| crate::core::Expression::Variable("_".to_string()));

        let executor = RollUpApplyExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            RollupApplyConfig {
                left_input_var,
                right_input_var,
                compare_cols,
                collect_col,
                col_names: node.col_names().to_vec(),
            },
        );
        Ok(ExecutorEnum::RollUpApply(executor))
    }

    /// Constructing the PatternApply executor
    pub fn build_pattern_apply(
        node: &PatternApplyNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let left_input_var = node
            .left_input_var()
            .cloned()
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_input_var = node
            .right_input_var()
            .cloned()
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let key_cols: Vec<crate::core::Expression> = node
            .key_cols()
            .iter()
            .filter_map(|ctx_expr| ctx_expr.get_expression())
            .collect();

        let executor = PatternApplyExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            PatternApplyConfig {
                left_input_var,
                right_input_var,
                key_cols,
                col_names: node.col_names().to_vec(),
                is_anti_predicate: node.is_anti_predicate(),
            },
        );
        Ok(ExecutorEnum::PatternApply(executor))
    }

    /// Constructing the Apply executor
    /// Apply executes a correlated subquery for each row from the left input
    pub fn build_apply(
        node: &ApplyNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let left_input_var = node
            .left_input_var()
            .cloned()
            .unwrap_or_else(|| format!("left_{}", node.id()));
        let right_input_var = node
            .right_input_var()
            .cloned()
            .unwrap_or_else(|| format!("right_{}", node.id()));

        let correlated_cols: Vec<crate::core::Expression> = node
            .correlated_cols()
            .iter()
            .map(|col| crate::core::Expression::Variable(col.clone()))
            .collect();

        let executor = PatternApplyExecutor::new(
            ExecutorConfig::new(node.id(), storage, context.expression_context().clone()),
            PatternApplyConfig {
                left_input_var,
                right_input_var,
                key_cols: correlated_cols,
                col_names: node.col_names().to_vec(),
                is_anti_predicate: node.is_anti(),
            },
        );
        Ok(ExecutorEnum::PatternApply(executor))
    }
}

impl<S: StorageClient + 'static> Default for TransformationBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

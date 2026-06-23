//! Data Processing Executor Builder
//!
//! Responsible for creating executors of data processing types (Filter, Project, Limit, Sort, TopN, Sample, Aggregate, Dedup).

use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::relational_algebra::{
    AggregateExecutor, AggregateFunctionSpec, FilterExecutor, ProjectExecutor, ProjectionColumn,
};
use crate::query::executor::result_processing::{
    DedupExecutor, LimitExecutor, SampleExecutor, SampleMethod, SortExecutor, SortKey, TopNExecutor,
};
use crate::query::planning::plan::core::nodes::{
    AggregateNode, DedupNode, FilterNode, LimitNode, ProjectNode, SampleNode, SortNode, TopNNode,
};
use crate::storage::StorageClient;
use parking_lot::RwLock;
use std::sync::Arc;

/// Data Processing Executor Builder
pub struct DataProcessingBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> DataProcessingBuilder<S> {
    /// Create a new data processing builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Building a Filter Executor
    pub fn build_filter(
        node: &FilterNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // The `FilterExecutor::new` method requires a `ContextualExpression`.
        let condition = node.condition().clone();

        let executor = FilterExecutor::new(node.id(), storage, condition);
        Ok(ExecutorEnum::Filter(executor))
    }

    /// Building the Project Executor
    pub fn build_project(
        node: &ProjectNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Convert YieldColumn to ProjectionColumn.
        // The expression field of the YieldColumn is a ContextualExpression.
        let columns: Vec<ProjectionColumn> = node
            .columns()
            .iter()
            .map(|col| ProjectionColumn::new(col.alias.clone(), col.expression.clone()))
            .collect();

        let executor = ProjectExecutor::new(
            node.id(),
            storage,
            columns,
            context.expression_context().clone(),
        );
        Ok(ExecutorEnum::Project(executor))
    }

    /// Building the Limit executor
    pub fn build_limit(
        node: &LimitNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Parameters of LimitExecutor::new: id, storage, limit, offset
        // 注意：LimitNode 的 offset() 和 count() 返回 i64，需要转换
        let executor = LimitExecutor::new(
            node.id(),
            storage,
            Some(node.count() as usize),
            node.offset() as usize,
        );
        Ok(ExecutorEnum::Limit(executor))
    }

    /// Building the Sort executor
    pub fn build_sort(
        node: &SortNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // SortItem now contains an Expression instead of just a column name
        let sort_keys: Vec<SortKey> = node
            .sort_items()
            .iter()
            .map(|item| {
                let order =
                    if item.direction == crate::core::types::graph_schema::OrderDirection::Desc {
                        crate::query::executor::result_processing::SortOrder::Desc
                    } else {
                        crate::query::executor::result_processing::SortOrder::Asc
                    };
                SortKey::new(item.expression.clone(), order)
            })
            .collect();

        use crate::query::executor::result_processing::SortConfig;
        let executor = SortExecutor::new(
            node.id(),
            storage,
            sort_keys,
            None, // limit
            SortConfig::default(),
        )
        .map_err(|e| QueryError::execution(e.to_string()))?;

        Ok(ExecutorEnum::Sort(executor))
    }

    /// Building a TopN executor
    pub fn build_topn(
        node: &TopNNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // TopNExecutor now supports expressions via with_sort_keys
        let sort_keys: Vec<SortKey> = node
            .sort_items()
            .iter()
            .map(|item| {
                let order =
                    if item.direction == crate::core::types::graph_schema::OrderDirection::Desc {
                        crate::query::executor::result_processing::SortOrder::Desc
                    } else {
                        crate::query::executor::result_processing::SortOrder::Asc
                    };
                SortKey::new(item.expression.clone(), order)
            })
            .collect();

        let executor =
            TopNExecutor::with_sort_keys(node.id(), storage, node.limit() as usize, sort_keys);
        Ok(ExecutorEnum::TopN(executor))
    }

    /// Building the Sample Executor
    pub fn build_sample(
        node: &SampleNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // Parameters for SampleExecutor::new: id, storage, method, count, seed
        let executor = SampleExecutor::new(
            node.id(),
            storage,
            SampleMethod::Random,
            node.count() as usize,
            None, // seed
        );
        Ok(ExecutorEnum::Sample(executor))
    }

    /// Building the Aggregate Executor
    pub fn build_aggregate(
        node: &AggregateNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        // `group_keys` is a `Vec<String>`, so it can be directly converted to an `Expression::Variable`.
        let group_keys: Vec<crate::core::Expression> = node
            .group_keys()
            .iter()
            .map(|key| crate::core::Expression::Variable(key.clone()))
            .collect();

        // The `AggregateFunction` needs to be converted to `Vec<AggregateFunctionSpec>`.
        let aggregate_functions: Vec<AggregateFunctionSpec> = node
            .aggregation_functions()
            .iter()
            .map(|agg_func| AggregateFunctionSpec::from_agg_function(agg_func.clone()))
            .collect();

        // Get column names from the node
        let col_names = node.col_names().to_vec();

        // Use with_col_names to pass column names
        let executor = AggregateExecutor::with_col_names(
            node.id(),
            storage,
            aggregate_functions,
            group_keys,
            col_names,
        );
        Ok(ExecutorEnum::Aggregate(executor))
    }

    /// Building a Dedup Executor
    pub fn build_dedup(
        node: &DedupNode,
        storage: Arc<RwLock<S>>,
        _context: &ExecutionContext,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        use crate::query::executor::result_processing::dedup::DedupStrategy;
        // The DedupNode does not have a “keys” field and uses a strategy for complete data deduplication (i.e., removing all duplicate data).
        let strategy = DedupStrategy::Full;

        let executor = DedupExecutor::new(
            node.id(),
            storage,
            strategy,
            None, // Use the default memory limit.
        );
        Ok(ExecutorEnum::Dedup(executor))
    }
}

impl<S: StorageClient + 'static> Default for DataProcessingBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}

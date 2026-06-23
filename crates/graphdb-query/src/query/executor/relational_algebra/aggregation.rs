//! Aggregation Operation Executor Module
//!
//! Executors related to aggregate operations, including:
//! - `GroupBy` (Grouping and Aggregation)
//! - Aggregate (overall aggregation)
//! - Having (filtered after grouping)
//!
//! CPU-intensive operations are parallelized using Rayon.
//!
//! Refer to the implementation of AggregateExecutor in nebula-graph:
//! - Use AggData to manage the aggregation status.
//! - Use the AggFunctionManager to manage aggregate functions.
//! - Unified handling of NULL and empty values

use parking_lot::RwLock;
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::types::operators::AggregateFunction;
use crate::core::value::{NullType, Value};
use crate::core::Expression;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, ExecutorStats};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::executor::result_processing::agg_data::AggData;
use crate::query::executor::result_processing::agg_function_manager::AggFunctionManager;
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::storage::StorageClient;

/// Aggregation function specifications
/// Includes the type of aggregate function and optional field name parameters.
#[derive(Debug, Clone)]
pub struct AggregateFunctionSpec {
    pub function: AggregateFunction,
    pub field: Option<String>,
    pub distinct: bool,
}

impl AggregateFunctionSpec {
    pub fn new(function: AggregateFunction) -> Self {
        Self {
            function,
            field: None,
            distinct: false,
        }
    }

    pub fn with_field(mut self, field: String) -> Self {
        self.field = Some(field);
        self
    }

    pub fn with_distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    // Convenient constructor
    pub fn count() -> Self {
        Self::new(AggregateFunction::Count(None))
    }

    pub fn count_with_field(field: String) -> Self {
        Self::new(AggregateFunction::Count(Some(field)))
    }

    pub fn count_distinct(field: String) -> Self {
        Self::new(AggregateFunction::Distinct(field))
    }

    pub fn sum(field: String) -> Self {
        Self::new(AggregateFunction::Sum(field))
    }

    pub fn avg(field: String) -> Self {
        Self {
            function: AggregateFunction::Avg(field.clone()),
            field: Some(field),
            distinct: false,
        }
    }

    pub fn max(field: String) -> Self {
        Self::new(AggregateFunction::Max(field))
    }

    pub fn min(field: String) -> Self {
        Self::new(AggregateFunction::Min(field))
    }

    pub fn collect(field: String) -> Self {
        Self::new(AggregateFunction::Collect(field))
    }

    pub fn collect_set(field: String) -> Self {
        Self::new(AggregateFunction::CollectSet(field))
    }

    /// Creating an AggregateFunctionSpec from an AggregateFunction
    pub fn from_agg_function(function: AggregateFunction) -> Self {
        let field = function.field_name().map(|s| s.to_string());
        Self {
            function,
            field,
            distinct: false,
        }
    }

    /// Obtain the names of the aggregate functions (for use with AggFunctionManager)
    pub fn agg_function_name(&self) -> String {
        let base_name = self.function.name().to_string();
        if self.distinct && matches!(self.function, AggregateFunction::Count(_)) {
            // COUNT DISTINCT counts the unique values after using COLLECT_SET to remove duplicates.
            "COLLECT_SET".to_string()
        } else {
            base_name
        }
    }
}

/// Group aggregation status (using the new AggData)
#[derive(Debug, Clone)]
pub struct GroupAggregateState {
    /// List of aggregated data corresponding to each group key
    /// Each aggregate function corresponds to an AggData object.
    pub groups: HashMap<Vec<Value>, Vec<AggData>>,
    /// Number of aggregate functions
    pub agg_func_count: usize,
}

impl GroupAggregateState {
    pub fn new(agg_func_count: usize) -> Self {
        Self {
            groups: HashMap::new(),
            agg_func_count,
        }
    }

    /// Obtaining or creating aggregated data for a group
    pub fn get_or_create_agg_data(&mut self, group_key: Vec<Value>) -> &mut Vec<AggData> {
        self.groups
            .entry(group_key)
            .or_insert_with(|| (0..self.agg_func_count).map(|_| AggData::new()).collect())
    }

    /// Merge another GroupAggregateState
    pub fn merge(&mut self, other: GroupAggregateState) -> DBResult<()> {
        for (group_key, other_agg_data_list) in other.groups {
            let self_agg_data_list = self.get_or_create_agg_data(group_key);
            for (i, other_agg_data) in other_agg_data_list.iter().enumerate() {
                if i < self_agg_data_list.len() {
                    Self::merge_agg_data(&mut self_agg_data_list[i], other_agg_data)?;
                }
            }
        }
        Ok(())
    }

    /// Merge the two AggData datasets.
    fn merge_agg_data(target: &mut AggData, source: &AggData) -> DBResult<()> {
        if !source.result().is_null() && !source.result().is_empty() {
            if target.result().is_null() || target.result().is_empty() {
                target.set_result(source.result().clone());
            } else if let Ok(new_value) = target.result().add(source.result()) {
                target.set_result(new_value);
            }
        }

        if !source.cnt().is_null() && !source.cnt().is_empty() {
            if target.cnt().is_null() || target.cnt().is_empty() {
                target.set_cnt(source.cnt().clone());
            } else if let Ok(new_value) = target.cnt().add(source.cnt()) {
                target.set_cnt(new_value);
            }
        }

        if !source.sum().is_null() && !source.sum().is_empty() {
            if target.sum().is_null() || target.sum().is_empty() {
                target.set_sum(source.sum().clone());
            } else if let Ok(new_value) = target.sum().add(source.sum()) {
                target.set_sum(new_value);
            }
        }

        if !source.avg().is_null()
            && !source.avg().is_empty()
            && (target.avg().is_null() || target.avg().is_empty())
        {
            target.set_avg(source.avg().clone());
        }

        Ok(())
    }
}

/// AggregateExecutor – The Aggregate Executor
///
/// Aggregation operations are supported, including aggregate functions such as COUNT, SUM, AVG, MAX, and MIN.
/// CPU-intensive operations are parallelized using Rayon.
pub struct AggregateExecutor<S: StorageClient + Send + 'static> {
    /// Basic processor
    base: BaseResultProcessor<S>,
    /// List of aggregate functions
    aggregate_functions: Vec<AggregateFunctionSpec>,
    /// List of grouping keys
    group_keys: Vec<Expression>,
    /// Column names for output
    col_names: Vec<String>,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Parallel computing configuration
    parallel_config: ParallelConfig,
    /// Aggregate Function Manager
    agg_function_manager: AggFunctionManager,
}

impl<S: StorageClient> AggregateExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        aggregate_functions: Vec<AggregateFunctionSpec>,
        group_keys: Vec<Expression>,
    ) -> Self {
        Self::with_col_names(id, storage, aggregate_functions, group_keys, Vec::new())
    }

    pub fn with_col_names(
        id: i64,
        storage: Arc<RwLock<S>>,
        aggregate_functions: Vec<AggregateFunctionSpec>,
        group_keys: Vec<Expression>,
        col_names: Vec<String>,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "AggregateExecutor".to_string(),
            "Performs aggregation operations on query results".to_string(),
            storage,
        );

        Self {
            base,
            aggregate_functions: aggregate_functions.clone(),
            group_keys,
            col_names,
            input_executor: None,
            parallel_config: ParallelConfig::default(),
            agg_function_manager: AggFunctionManager::new(),
        }
    }

    /// Setting up parallel computing configuration
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    fn process_input(&mut self) -> DBResult<crate::query::DataSet> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else if let Some(input) = &self.base.input {
            input.clone()
        } else {
            return Err(DBError::query(
                "Aggregate executor requires input executor".to_string(),
            ));
        };

        match input_result {
            ExecutionResult::DataSet(dataset) => self.aggregate_dataset(dataset),
            ExecutionResult::Empty
            | ExecutionResult::Success
            | ExecutionResult::SpaceSwitched(_) => {
                let dataset = crate::query::DataSet::new();
                self.aggregate_dataset(dataset)
            }
            ExecutionResult::Error(msg) => Err(DBError::query(msg)),
        }
    }

    /// Convert Vertices to a DataSet for aggregation
    fn _vertices_to_dataset(&self, vertices: Vec<crate::core::Vertex>) -> crate::query::DataSet {
        let mut dataset = crate::query::DataSet::new();
        // Use the first group key as the column name, or default to "vertex"
        let col_name = self
            .group_keys
            .first()
            .and_then(|expr| match expr {
                Expression::Variable(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "vertex".to_string());
        dataset.col_names = vec![col_name];

        for vertex in vertices {
            let row = vec![crate::core::Value::Vertex(Box::new(vertex))];
            dataset.rows.push(row);
        }

        dataset
    }

    /// Convert Edges to a DataSet for aggregation
    fn _edges_to_dataset(&self, edges: Vec<crate::core::Edge>) -> crate::query::DataSet {
        let mut dataset = crate::query::DataSet::new();
        // Use the first group key as the column name, or default to "edge"
        let col_name = self
            .group_keys
            .first()
            .and_then(|expr| match expr {
                Expression::Variable(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "edge".to_string());
        dataset.col_names = vec![col_name];

        for edge in edges {
            let row = vec![crate::core::Value::edge(edge)];
            dataset.rows.push(row);
        }

        dataset
    }

    fn aggregate_dataset(
        &mut self,
        dataset: crate::query::DataSet,
    ) -> DBResult<crate::query::DataSet> {
        let total_size = dataset.rows.len();

        // 处理 COUNT(*) 的特殊情况（无分组键且只有一个 COUNT(*)）
        if self.group_keys.is_empty()
            && self.aggregate_functions.len() == 1
            && matches!(
                self.aggregate_functions[0].function,
                AggregateFunction::Count(None)
            )
        {
            return self.handle_count_star(dataset);
        }

        if self.parallel_config.should_use_parallel(total_size) {
            self.aggregate_dataset_parallel(dataset)
        } else {
            self.aggregate_dataset_serial(dataset)
        }
    }

    /// 处理 COUNT(*) 特殊情况
    fn handle_count_star(&self, dataset: crate::query::DataSet) -> DBResult<crate::query::DataSet> {
        let mut result_dataset = crate::query::DataSet::new();
        result_dataset.col_names.push("count".to_string());
        result_dataset
            .rows
            .push(vec![Value::BigInt(dataset.rows.len() as i64)]);
        Ok(result_dataset)
    }

    fn aggregate_dataset_serial(
        &mut self,
        dataset: crate::query::DataSet,
    ) -> DBResult<crate::query::DataSet> {
        let agg_func_count = self.aggregate_functions.len();
        let mut group_state = GroupAggregateState::new(agg_func_count);

        // Process each row of data.
        for row in &dataset.rows {
            // Constructing the context for the expression
            let mut context = DefaultExpressionContext::new();
            for (i, col_name) in dataset.col_names.iter().enumerate() {
                if i < row.len() {
                    context.set_variable(col_name.clone(), row[i].clone());
                }
            }

            // Calculate the grouping key
            let group_key: Vec<Value> = self
                .group_keys
                .iter()
                .map(|expr| {
                    ExpressionEvaluator::evaluate(expr, &mut context)
                        .unwrap_or(Value::Null(NullType::NaN))
                })
                .collect();

            // Obtaining or creating aggregated data
            let agg_data_list = group_state.get_or_create_agg_data(group_key);

            // Evaluate each aggregate function.
            for (i, agg_func) in self.aggregate_functions.iter().enumerate() {
                if i >= agg_data_list.len() {
                    continue;
                }

                let agg_data = &mut agg_data_list[i];
                let value = self.get_value_for_agg(&mut context, agg_func, row, &dataset.col_names);

                // Obtain the aggregate functions and execute them.
                let func_name = agg_func.agg_function_name();
                if let Some(agg_fn) = self.agg_function_manager.get(&func_name) {
                    agg_fn(agg_data, &value)?;
                }
            }
        }

        // Constructing the resulting dataset
        self.build_result_dataset(group_state)
    }

    /// Extract a property value from a Vertex or Edge value
    fn extract_property_from_value(value: &Value, property: &str) -> Value {
        match value {
            Value::Vertex(vertex) => {
                if let Some(val) = vertex.properties.get(property) {
                    return val.clone();
                }
                for tag in &vertex.tags {
                    if let Some(val) = tag.properties.get(property) {
                        return val.clone();
                    }
                }
                Value::Null(NullType::Null)
            }
            Value::Edge(edge) => edge
                .properties()
                .get(property)
                .cloned()
                .unwrap_or(Value::Null(NullType::Null)),
            _ => Value::Null(NullType::Null),
        }
    }

    /// Try to resolve a field name that may be in "var.property" format
    fn resolve_field_value(
        context: &mut DefaultExpressionContext,
        field: &str,
        row: &[Value],
        col_names: &[String],
    ) -> Value {
        // First try direct lookup
        if let Some(val) = context.get_variable(field) {
            return val;
        }
        if let Some(col_index) = col_names.iter().position(|name| name == field) {
            if col_index < row.len() {
                return row[col_index].clone();
            }
        }

        // Try dotted format: "var.property"
        if let Some(dot_pos) = field.find('.') {
            let var_name = &field[..dot_pos];
            let property = &field[dot_pos + 1..];

            if let Some(val) = context.get_variable(var_name) {
                return Self::extract_property_from_value(&val, property);
            }
            if let Some(col_index) = col_names.iter().position(|name| name == var_name) {
                if col_index < row.len() {
                    return Self::extract_property_from_value(&row[col_index], property);
                }
            }
        }

        Value::Null(NullType::Null)
    }

    /// Obtaining the values required for the aggregate functions
    fn get_value_for_agg(
        &self,
        context: &mut DefaultExpressionContext,
        agg_func: &AggregateFunctionSpec,
        row: &[Value],
        col_names: &[String],
    ) -> Value {
        match &agg_func.function {
            AggregateFunction::Count(None) => {
                // COUNT(*) - 计数 1
                Value::Int(1)
            }
            AggregateFunction::Count(Some(field))
            | AggregateFunction::Sum(field)
            | AggregateFunction::Avg(field)
            | AggregateFunction::Max(field)
            | AggregateFunction::Min(field)
            | AggregateFunction::Collect(field)
            | AggregateFunction::CollectSet(field)
            | AggregateFunction::Distinct(field)
            | AggregateFunction::Std(field)
            | AggregateFunction::BitAnd(field)
            | AggregateFunction::BitOr(field) => {
                Self::resolve_field_value(context, field, row, col_names)
            }
            AggregateFunction::Percentile(field, _) => {
                Self::resolve_field_value(context, field, row, col_names)
            }
            AggregateFunction::GroupConcat(field, _) => {
                Self::resolve_field_value(context, field, row, col_names)
            }
            AggregateFunction::VecSum(field) => {
                Self::resolve_field_value(context, field, row, col_names)
            }
            AggregateFunction::VecAvg(field) => {
                Self::resolve_field_value(context, field, row, col_names)
            }
        }
    }

    /// Parallel aggregation
    fn aggregate_dataset_parallel(
        &mut self,
        dataset: crate::query::DataSet,
    ) -> DBResult<crate::query::DataSet> {
        let batch_size = self
            .parallel_config
            .calculate_batch_size(dataset.rows.len());
        let aggregate_functions = self.aggregate_functions.clone();
        let group_keys = self.group_keys.clone();
        let col_names = dataset.col_names.clone();
        let agg_function_manager = self.agg_function_manager.clone();

        // Use rayon to process data batches in parallel.
        let partial_results: Vec<GroupAggregateState> = dataset
            .rows
            .par_chunks(batch_size)
            .map(|chunk| {
                let agg_func_count = aggregate_functions.len();
                let mut local_state = GroupAggregateState::new(agg_func_count);

                for row in chunk {
                    // Building the context for the expression
                    let mut context = DefaultExpressionContext::new();
                    for (i, col_name) in col_names.iter().enumerate() {
                        if i < row.len() {
                            context.set_variable(col_name.clone(), row[i].clone());
                        }
                    }

                    // Calculate the grouping key
                    let group_key: Vec<Value> = group_keys
                        .iter()
                        .map(|expr| {
                            ExpressionEvaluator::evaluate(expr, &mut context)
                                .unwrap_or(Value::Null(NullType::NaN))
                        })
                        .collect();

                    // Obtaining or creating aggregated data
                    let agg_data_list = local_state.get_or_create_agg_data(group_key);

                    // Evaluate each aggregate function.
                    for (i, agg_func) in aggregate_functions.iter().enumerate() {
                        if i >= agg_data_list.len() {
                            continue;
                        }

                        let agg_data = &mut agg_data_list[i];

                        // Obtain the value
                        let value = match &agg_func.function {
                            AggregateFunction::Count(None) => Value::Int(1),
                            AggregateFunction::Count(Some(field))
                            | AggregateFunction::Sum(field)
                            | AggregateFunction::Avg(field)
                            | AggregateFunction::Max(field)
                            | AggregateFunction::Min(field)
                            | AggregateFunction::Collect(field)
                            | AggregateFunction::CollectSet(field)
                            | AggregateFunction::Distinct(field)
                            | AggregateFunction::Std(field)
                            | AggregateFunction::BitAnd(field)
                            | AggregateFunction::BitOr(field) => {
                                // Try direct lookup first
                                if let Some(col_index) =
                                    col_names.iter().position(|name| name == field)
                                {
                                    if col_index < row.len() {
                                        row[col_index].clone()
                                    } else {
                                        Value::Null(NullType::Null)
                                    }
                                } else if let Some(dot_pos) = field.find('.') {
                                    let var_name = &field[..dot_pos];
                                    let property = &field[dot_pos + 1..];
                                    if let Some(col_index) =
                                        col_names.iter().position(|name| name == var_name)
                                    {
                                        if col_index < row.len() {
                                            let val = &row[col_index];
                                            Self::extract_property_from_value(val, property)
                                        } else {
                                            Value::Null(NullType::Null)
                                        }
                                    } else {
                                        Value::Null(NullType::Null)
                                    }
                                } else {
                                    Value::Null(NullType::Null)
                                }
                            }
                            _ => Value::Null(NullType::Null),
                        };

                        // Retrieve the aggregate functions and execute them.
                        let func_name = agg_func.agg_function_name();
                        if let Some(agg_fn) = agg_function_manager.get(&func_name) {
                            let _ = agg_fn(agg_data, &value);
                        }
                    }
                }

                local_state
            })
            .collect();

        // Gather: Merge all the local aggregation results.
        let agg_func_count = self.aggregate_functions.len();
        let mut global_state = GroupAggregateState::new(agg_func_count);
        for partial_state in partial_results {
            global_state.merge(partial_state)?;
        }

        // Constructing the resulting dataset
        self.build_result_dataset(global_state)
    }

    fn build_result_dataset(
        &self,
        group_state: GroupAggregateState,
    ) -> DBResult<crate::query::DataSet> {
        let mut result_dataset = crate::query::DataSet::new();

        // Set column names - use provided col_names if available
        if !self.col_names.is_empty() {
            result_dataset.col_names = self.col_names.clone();
        } else {
            // Fallback to generating column names
            for _ in &self.group_keys {
                result_dataset
                    .col_names
                    .push(format!("group_{}", result_dataset.col_names.len()));
            }

            for agg_func in &self.aggregate_functions {
                let col_name = match &agg_func.function {
                    AggregateFunction::Count(_) => {
                        if agg_func.distinct {
                            if let Some(ref field) = agg_func.field {
                                format!("count_distinct_{}", field)
                            } else {
                                "count_distinct".to_string()
                            }
                        } else if let Some(ref field) = agg_func.field {
                            format!("count_{}", field)
                        } else {
                            "count".to_string()
                        }
                    }
                    AggregateFunction::Sum(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("sum_{}", field)
                        } else {
                            "sum".to_string()
                        }
                    }
                    AggregateFunction::Avg(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("avg_{}", field)
                        } else {
                            "avg".to_string()
                        }
                    }
                    AggregateFunction::Max(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("max_{}", field)
                        } else {
                            "max".to_string()
                        }
                    }
                    AggregateFunction::Min(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("min_{}", field)
                        } else {
                            "min".to_string()
                        }
                    }
                    AggregateFunction::Collect(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("collect_{}", field)
                        } else {
                            "collect".to_string()
                        }
                    }
                    AggregateFunction::CollectSet(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("collect_set_{}", field)
                        } else {
                            "collect_set".to_string()
                        }
                    }
                    AggregateFunction::Distinct(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("distinct_{}", field)
                        } else {
                            "distinct".to_string()
                        }
                    }
                    AggregateFunction::Percentile(_, _) => {
                        if let Some(ref field) = agg_func.field {
                            format!("percentile_{}", field)
                        } else {
                            "percentile".to_string()
                        }
                    }
                    AggregateFunction::Std(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("std_{}", field)
                        } else {
                            "std".to_string()
                        }
                    }
                    AggregateFunction::BitAnd(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("bitand_{}", field)
                        } else {
                            "bitand".to_string()
                        }
                    }
                    AggregateFunction::BitOr(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("bitor_{}", field)
                        } else {
                            "bitor".to_string()
                        }
                    }
                    AggregateFunction::GroupConcat(_, _) => {
                        if let Some(ref field) = agg_func.field {
                            format!("group_concat_{}", field)
                        } else {
                            "group_concat".to_string()
                        }
                    }
                    AggregateFunction::VecSum(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("vecsum_{}", field)
                        } else {
                            "vecsum".to_string()
                        }
                    }
                    AggregateFunction::VecAvg(_) => {
                        if let Some(ref field) = agg_func.field {
                            format!("vecavg_{}", field)
                        } else {
                            "vecavg".to_string()
                        }
                    }
                };
                result_dataset.col_names.push(col_name);
            }
        }

        // Fill in the result rows
        for (group_key, agg_data_list) in &group_state.groups {
            let mut result_row = Vec::new();

            // Add group key-value pairs
            result_row.extend_from_slice(group_key);

            // Add the aggregated results.
            for (i, agg_func) in self.aggregate_functions.iter().enumerate() {
                if i < agg_data_list.len() {
                    let agg_data = &agg_data_list[i];

                    // Handling special cases of COUNT DISTINCT
                    let agg_value = if agg_func.distinct
                        && matches!(agg_func.function, AggregateFunction::Count(_))
                    {
                        if let Some(uniques) = agg_data.uniques() {
                            Value::BigInt(uniques.len() as i64)
                        } else {
                            Value::BigInt(0)
                        }
                    } else {
                        agg_data.result().clone()
                    };

                    result_row.push(agg_value);
                } else {
                    result_row.push(Value::Null(NullType::NaN));
                }
            }

            result_dataset.rows.push(result_row);
        }

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for AggregateExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        ResultProcessor::set_input(self, input);
        let dataset = self.process_input()?;
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn set_input(&mut self, input: ExecutionResult) {
        self.base.input = Some(input);
    }

    fn get_input(&self) -> Option<&ExecutionResult> {
        self.base.input.as_ref()
    }

    fn context(&self) -> &ResultProcessorContext {
        &self.base.context
    }

    fn set_context(&mut self, context: ResultProcessorContext) {
        self.base.context = context;
    }

    fn memory_usage(&self) -> usize {
        self.base.memory_usage
    }

    fn reset(&mut self) {
        self.base.reset_state();
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for AggregateExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(crate::query::DataSet::new()))
        };

        self.process(input_result)
    }

    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.id > 0
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

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for AggregateExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

/// GroupByExecutor – An executor for grouping and aggregating data
///
/// Implementing the GROUP BY operation
pub struct GroupByExecutor<S: StorageClient + Send + 'static> {
    aggregate_executor: AggregateExecutor<S>,
}

impl<S: StorageClient + Send + 'static> GroupByExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        aggregate_functions: Vec<AggregateFunctionSpec>,
        group_keys: Vec<Expression>,
    ) -> Self {
        Self {
            aggregate_executor: AggregateExecutor::new(
                id,
                storage,
                aggregate_functions,
                group_keys,
            ),
        }
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for GroupByExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        InputExecutor::set_input(&mut self.aggregate_executor, input);
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        InputExecutor::get_input(&self.aggregate_executor)
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for GroupByExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        self.aggregate_executor.execute()
    }

    fn open(&mut self) -> DBResult<()> {
        self.aggregate_executor.open()
    }

    fn close(&mut self) -> DBResult<()> {
        self.aggregate_executor.close()
    }

    fn is_open(&self) -> bool {
        self.aggregate_executor.is_open()
    }

    fn id(&self) -> i64 {
        self.aggregate_executor.id()
    }

    fn name(&self) -> &str {
        "GroupByExecutor"
    }

    fn description(&self) -> &str {
        "GroupByExecutor - performs GROUP BY operations"
    }

    fn stats(&self) -> &ExecutorStats {
        self.aggregate_executor.stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.aggregate_executor.stats_mut()
    }
}

/// HavingExecutor – The executor for the HAVING clause
///
/// Implementing the HAVING clause to filter the results after grouping
pub struct HavingExecutor<S: StorageClient + Send + 'static> {
    /// Basic Processor
    base: BaseResultProcessor<S>,
    /// HAVING conditional expression
    condition: Expression,
    /// Input Actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient> HavingExecutor<S> {
    pub fn new(id: i64, storage: Arc<RwLock<S>>, condition: Expression) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "HavingExecutor".to_string(),
            "Filters grouped results using HAVING clause".to_string(),
            storage,
        );

        Self {
            base,
            condition,
            input_executor: None,
        }
    }

    fn process_input(&mut self) -> DBResult<crate::query::DataSet> {
        if let Some(ref mut input_exec) = self.input_executor {
            let input_result = input_exec.execute()?;

            match input_result {
                ExecutionResult::DataSet(mut dataset) => {
                    self.apply_having_condition(&mut dataset)?;
                    Ok(dataset)
                }
                _ => Err(DBError::query(
                    "Having executor expects DataSet input".to_string(),
                )),
            }
        } else if let Some(input) = &self.base.input {
            match input {
                ExecutionResult::DataSet(dataset) => {
                    let mut dataset = dataset.clone();
                    self.apply_having_condition(&mut dataset)?;
                    Ok(dataset)
                }
                _ => Err(DBError::query(
                    "Having executor expects DataSet input".to_string(),
                )),
            }
        } else {
            Err(DBError::query(
                "Having executor requires input executor".to_string(),
            ))
        }
    }

    fn apply_having_condition(&self, dataset: &mut crate::query::DataSet) -> DBResult<()> {
        let mut filtered_rows = Vec::new();

        for row in &dataset.rows {
            // Constructing the context for the expression
            let mut context = DefaultExpressionContext::new();
            for (i, col_name) in dataset.col_names.iter().enumerate() {
                if i < row.len() {
                    context.set_variable(col_name.clone(), row[i].clone());
                }
            }

            // Evaluating the HAVING condition
            match ExpressionEvaluator::evaluate(&self.condition, &mut context) {
                Ok(Value::Bool(true)) => {
                    filtered_rows.push(row.clone());
                }
                Ok(Value::Bool(false)) => {
                    // If the condition is “false”, skip that line.
                }
                Ok(_) => {
                    // Non-boolean values are considered false.
                }
                Err(e) => {
                    return Err(DBError::query(format!(
                        "Failed to evaluate HAVING condition: {}",
                        e
                    )));
                }
            }
        }

        dataset.rows = filtered_rows;
        Ok(())
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for HavingExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        ResultProcessor::set_input(self, input);
        let dataset = self.process_input()?;
        Ok(ExecutionResult::DataSet(dataset))
    }

    fn set_input(&mut self, input: ExecutionResult) {
        self.base.input = Some(input);
    }

    fn get_input(&self) -> Option<&ExecutionResult> {
        self.base.input.as_ref()
    }

    fn context(&self) -> &ResultProcessorContext {
        &self.base.context
    }

    fn set_context(&mut self, context: ResultProcessorContext) {
        self.base.context = context;
    }

    fn memory_usage(&self) -> usize {
        self.base.memory_usage
    }

    fn reset(&mut self) {
        self.base.reset_state();
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for HavingExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(crate::query::DataSet::new()))
        };

        self.process(input_result)
    }

    fn open(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.open()?;
        }
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        if let Some(ref mut input_exec) = self.input_executor {
            input_exec.close()?;
        }
        Ok(())
    }

    fn is_open(&self) -> bool {
        self.base.id > 0
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

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for HavingExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

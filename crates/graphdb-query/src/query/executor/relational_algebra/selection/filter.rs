//! Filter executor
//!
//! Implementing a function for conditional filtering of search results, with support for the HAVING clause.
//! CPU-intensive operations are parallelized using Rayon.

use parking_lot::RwLock;
use rayon::prelude::*;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::ContextualExpression;
use crate::core::value::NullType;
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{ExecutionResult, Executor};
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::{DefaultExpressionContext, ExpressionContext};
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::query::DataSet;
use crate::storage::StorageClient;

fn _extract_variable_names(expr: &Expression) -> Vec<String> {
    let mut names = Vec::new();
    fn collect(expr: &Expression, names: &mut Vec<String>) {
        match expr {
            Expression::Variable(name) => {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            Expression::Property { object, .. } => collect(object, names),
            Expression::Binary { left, right, .. } => {
                collect(left, names);
                collect(right, names);
            }
            Expression::Unary { operand, .. } => collect(operand, names),
            Expression::Function { args, .. } => {
                for arg in args {
                    collect(arg, names);
                }
            }
            Expression::Aggregate { arg, .. } => collect(arg, names),
            Expression::List(elements) => {
                for elem in elements {
                    collect(elem, names);
                }
            }
            Expression::Map(entries) => {
                for (_, val_expr) in entries {
                    collect(val_expr, names);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(te) = test_expr {
                    collect(te, names);
                }
                for (cond, val) in conditions {
                    collect(cond, names);
                    collect(val, names);
                }
                if let Some(d) = default {
                    collect(d, names);
                }
            }
            Expression::TypeCast { expression, .. } => collect(expression, names),
            Expression::Subscript { collection, index } => {
                collect(collection, names);
                collect(index, names);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                collect(collection, names);
                if let Some(s) = start {
                    collect(s, names);
                }
                if let Some(e) = end {
                    collect(e, names);
                }
            }
            Expression::Path(elements) => {
                for elem in elements {
                    collect(elem, names);
                }
            }
            Expression::LabelTagProperty { tag, .. } => collect(tag, names),
            Expression::Predicate { args, .. } => {
                for arg in args {
                    collect(arg, names);
                }
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                collect(initial, names);
                collect(source, names);
                collect(mapping, names);
            }
            Expression::PathBuild(elements) => {
                for elem in elements {
                    collect(elem, names);
                }
            }
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                collect(source, names);
                if let Some(f) = filter {
                    collect(f, names);
                }
                if let Some(m) = map {
                    collect(m, names);
                }
            }
            Expression::Literal(_)
            | Expression::Label(_)
            | Expression::TagProperty { .. }
            | Expression::EdgeProperty { .. }
            | Expression::Parameter(_)
            | Expression::Vector(_) => {}
        }
    }
    collect(expr, &mut names);
    names
}

const _INTERNAL_VARIABLES: &[&str] = &[
    "_vertex",
    "_edge",
    "id",
    "value",
    "row",
    "src",
    "dst",
    "edge_type",
    "ranking",
];

/// FilterExecutor – The filter execution component
///
/// Implementing the functionality to filter query results based on certain conditions
/// CPU-intensive operations are parallelized using Rayon.
pub struct FilterExecutor<S: StorageClient + Send + 'static> {
    /// Basic processor
    base: BaseResultProcessor<S>,
    /// Filter condition expression
    condition: Expression,
    /// Input actuator
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Parallel computing configuration
    parallel_config: ParallelConfig,
}

impl<S: StorageClient + Send + 'static> FilterExecutor<S> {
    pub fn new(id: i64, storage: Arc<RwLock<S>>, condition: ContextualExpression) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "FilterExecutor".to_string(),
            "Filters query results based on specified conditions".to_string(),
            storage,
        );

        // Extract the Expression from the ContextualExpression.
        let expr = Self::extract_expression(&condition);

        Self {
            base,
            condition: expr,
            input_executor: None,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Auxiliary method for extracting an Expression from a ContextualExpression
    fn extract_expression(ctx_expr: &ContextualExpression) -> Expression {
        match ctx_expr.expression() {
            Some(meta) => meta.inner().clone(),
            None => Expression::Literal(Value::Null(NullType::Null)),
        }
    }

    /// Setting up parallel computing configurations
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    /// Process the input data and apply the filtering criteria.
    fn process_input(&mut self) -> DBResult<ExecutionResult> {
        if let Some(ref mut input_exec) = self.input_executor {
            let input_result = input_exec.execute()?;
            self.filter_input(input_result)
        } else if let Some(input) = &self.base.input {
            self.filter_input(input.clone())
        } else {
            Err(DBError::query("Filter executor requires input".to_string()))
        }
    }

    /// Filter the input data.
    fn filter_input(&self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        match input {
            ExecutionResult::DataSet(mut dataset) => {
                self.apply_filter(&mut dataset)?;
                Ok(ExecutionResult::DataSet(dataset))
            }
            _ => Ok(input),
        }
    }

    /// Apply filtering criteria to the dataset.
    ///
    /// Select the filtering method based on the configuration:
    /// Data volume is below the threshold: Processing is done in a single thread.
    /// Large amount of data: Rayon is used for parallel processing.
    fn apply_filter(&self, dataset: &mut DataSet) -> DBResult<()> {
        let total_size = dataset.rows.len();

        // Determine whether to use parallel computing based on the parallel configuration.
        if !self.parallel_config.should_use_parallel(total_size) {
            // If the amount of data is small or parallel processing is disabled, single-threaded processing should be used.
            self.apply_filter_single(dataset)
        } else {
            // Given the large volume of data, Rayon is used for parallel processing.
            let batch_size = self.parallel_config.calculate_batch_size(total_size);
            self.apply_filter_parallel(dataset, batch_size)
        }
    }

    /// Single-threaded filtering
    fn apply_filter_single(&self, dataset: &mut DataSet) -> DBResult<()> {
        let mut filtered_rows = Vec::new();

        for row in &dataset.rows {
            let mut context = DefaultExpressionContext::new();

            // Set the column names as variables.
            for (i, col_name) in dataset.col_names.iter().enumerate() {
                if i < row.len() {
                    context.set_variable(col_name.clone(), row[i].clone());
                }
            }

            // Map GO query special variables: $$ -> dst, $^ -> src, target -> dst, edge -> edge
            if let Some(dst_idx) = dataset.col_names.iter().position(|c| c == "dst") {
                if dst_idx < row.len() {
                    context.set_variable("$$".to_string(), row[dst_idx].clone());
                    context.set_variable("target".to_string(), row[dst_idx].clone());
                }
            }
            if let Some(src_idx) = dataset.col_names.iter().position(|c| c == "src") {
                if src_idx < row.len() {
                    context.set_variable("$^".to_string(), row[src_idx].clone());
                }
            }
            if let Some(edge_idx) = dataset.col_names.iter().position(|c| c == "edge") {
                if edge_idx < row.len() {
                    context.set_variable("edge".to_string(), row[edge_idx].clone());
                    // Map edge type name to the edge value for GO queries like WHERE friend.strength > 5
                    if let crate::core::Value::Edge(ref edge_val) = row[edge_idx] {
                        context
                            .set_variable(edge_val.edge_type().to_string(), row[edge_idx].clone());
                    }
                }
            }

            // Handle table.column format: create table map variables
            let mut table_maps: std::collections::HashMap<
                String,
                std::collections::HashMap<String, crate::core::Value>,
            > = std::collections::HashMap::new();
            for (i, col_name) in dataset.col_names.iter().enumerate() {
                if i < row.len() {
                    if let Some(dot_pos) = col_name.find('.') {
                        let table = &col_name[..dot_pos];
                        let column = &col_name[dot_pos + 1..];
                        table_maps
                            .entry(table.to_string())
                            .or_default()
                            .insert(column.to_string(), row[i].clone());
                    }
                }
            }
            for (table, map) in table_maps {
                context.set_variable(table, crate::core::Value::map(map));
            }

            // Set the `row` variable (which contains the entire row of data)
            let row_map: std::collections::HashMap<String, crate::core::Value> = dataset
                .col_names
                .iter()
                .enumerate()
                .filter_map(|(i, name)| {
                    if i < row.len() {
                        Some((name.clone(), row[i].clone()))
                    } else {
                        None
                    }
                })
                .collect();
            context.set_variable("row".to_string(), crate::core::Value::map(row_map));

            let condition_result = ExpressionEvaluator::evaluate(&self.condition, &mut context)
                .map_err(|e| {
                    DBError::expression(format!("Failed to evaluate filter condition: {}", e))
                })?;

            if let crate::core::Value::Bool(true) = condition_result {
                filtered_rows.push(row.clone());
            }
        }

        dataset.rows = filtered_rows;
        Ok(())
    }

    /// Parallel filtering
    fn apply_filter_parallel(&self, dataset: &mut DataSet, batch_size: usize) -> DBResult<()> {
        let col_names = dataset.col_names.clone();
        let condition = self.condition.clone();

        let filtered_rows: Vec<Vec<Value>> = dataset
            .rows
            .par_chunks(batch_size)
            .flat_map(|chunk| {
                chunk
                    .iter()
                    .filter_map(|row| {
                        let mut context = DefaultExpressionContext::new();
                        for (i, col_name) in col_names.iter().enumerate() {
                            if i < row.len() {
                                context.set_variable(col_name.clone(), row[i].clone());
                            }
                        }

                        // Map GO query special variables: $$ -> dst, $^ -> src, target -> dst, edge -> edge
                        if let Some(dst_idx) = col_names.iter().position(|c| c == "dst") {
                            if dst_idx < row.len() {
                                context.set_variable("$$".to_string(), row[dst_idx].clone());
                                context.set_variable("target".to_string(), row[dst_idx].clone());
                            }
                        }
                        if let Some(src_idx) = col_names.iter().position(|c| c == "src") {
                            if src_idx < row.len() {
                                context.set_variable("$^".to_string(), row[src_idx].clone());
                            }
                        }
                        if let Some(edge_idx) = col_names.iter().position(|c| c == "edge") {
                            if edge_idx < row.len() {
                                context.set_variable("edge".to_string(), row[edge_idx].clone());
                                // Map edge type name to the edge value for GO queries like WHERE friend.strength > 5
                                if let crate::core::Value::Edge(ref edge_val) = row[edge_idx] {
                                    context.set_variable(
                                        edge_val.edge_type().to_string(),
                                        row[edge_idx].clone(),
                                    );
                                }
                            }
                        }

                        match ExpressionEvaluator::evaluate(&condition, &mut context) {
                            Ok(crate::core::Value::Bool(true)) => Some(row.clone()),
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        dataset.rows = filtered_rows;
        Ok(())
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for FilterExecutor<S> {
    fn process(&mut self, input: ExecutionResult) -> DBResult<ExecutionResult> {
        if self.input_executor.is_none() && self.base.input.is_none() {
            <Self as ResultProcessor>::set_input(self, input.clone());
        }
        self.process_input()
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

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for FilterExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            self.base
                .input
                .clone()
                .unwrap_or(ExecutionResult::DataSet(crate::query::DataSet::new()))
        };

        let result = self.process(input_result);

        if let Ok(ref exec_result) = result {
            self.base.get_stats_mut().add_row(exec_result.count());
        }

        result
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

impl<S: StorageClient + Send + 'static> InputExecutor<S> for FilterExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::validator::context::expression_context::ExpressionAnalysisContext;
    use crate::storage::MockStorage;

    #[test]
    fn test_filter_executor_basic() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));

        // Create test data
        let mut dataset = DataSet::new();
        dataset.col_names = vec!["name".to_string(), "age".to_string()];
        dataset.rows.push(vec![
            crate::core::Value::String("Alice".to_string()),
            crate::core::Value::Int(30),
        ]);
        dataset.rows.push(vec![
            crate::core::Value::String("Bob".to_string()),
            crate::core::Value::Int(25),
        ]);
        dataset.rows.push(vec![
            crate::core::Value::String("Charlie".to_string()),
            crate::core::Value::Int(35),
        ]);

        // Create a filter executor (age > 25)
        let condition = Expression::Binary {
            left: Box::new(Expression::Property {
                object: Box::new(Expression::Variable("row".to_string())),
                property: "age".to_string(),
            }),
            op: crate::core::BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(crate::core::Value::Int(25))),
        };

        use std::sync::Arc;
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let condition_meta = crate::core::types::ExpressionMeta::new(condition);
        let condition_id = ctx.register_expression(condition_meta);
        let ctx_condition = ContextualExpression::new(condition_id, ctx);

        let mut executor = FilterExecutor::new(1, storage, ctx_condition);

        // Setting the input data
        <FilterExecutor<MockStorage> as ResultProcessor>::set_input(
            &mut executor,
            ExecutionResult::DataSet(dataset),
        );

        // Please provide the text you would like to have translated. I will then perform the translation and remove any unnecessary elements (such as filters) from the resulting text.
        let result = executor
            .process(ExecutionResult::DataSet(DataSet::new()))
            .expect("Failed to get next");

        // Verification results
        match result {
            ExecutionResult::DataSet(filtered_dataset) => {
                assert_eq!(filtered_dataset.rows.len(), 2); // Alice and Charlie
                                                            // Verify that all ages are greater than 25.
                for row in &filtered_dataset.rows {
                    if let crate::core::Value::Int(age) = &row[1] {
                        assert!(*age > 25);
                    }
                }
            }
            _ => panic!("Expected DataSet result"),
        }
    }
}

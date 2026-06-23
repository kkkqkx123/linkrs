//! Column Projection Executor
//!
//! ProjectExecutor – Selection and projection of output columns
//!
//! CPU-intensive operations are parallelized using Rayon.

use parking_lot::RwLock;
use rayon::prelude::*;
use std::sync::Arc;

use crate::core::error::{DBError, DBResult};
use crate::core::types::ContextualExpression;
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::base::BaseExecutor;
use crate::query::executor::base::Executor;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::executor::utils::recursion_detector::ParallelConfig;
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::ExecutionResult;
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

/// Projection column definition
#[derive(Debug, Clone)]
pub struct ProjectionColumn {
    pub name: String,                     // Column names
    pub expression: ContextualExpression, // Projection expression
}

impl ProjectionColumn {
    pub fn new(name: String, expression: ContextualExpression) -> Self {
        Self { name, expression }
    }
}

/// ProjectExecutor – The projection executor
///
/// Performs column projection operations, supports the evaluation of expressions, and allows for the renaming of columns.
///
/// CPU-intensive operations are parallelized using Rayon.
pub struct ProjectExecutor<S: StorageClient + Send + 'static> {
    base: BaseExecutor<S>,
    columns: Vec<ProjectionColumn>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
    /// Parallel computing configuration
    parallel_config: ParallelConfig,
}

impl<S: StorageClient> ProjectExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        columns: Vec<ProjectionColumn>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "ProjectExecutor".to_string(), storage, expr_context),
            columns,
            input_executor: None,
            parallel_config: ParallelConfig::default(),
        }
    }

    /// Setting up parallel computing configuration
    pub fn with_parallel_config(mut self, config: ParallelConfig) -> Self {
        self.parallel_config = config;
        self
    }

    /// Projection of single-row data
    fn project_row(&self, row: &[Value], col_names: &[String]) -> DBResult<Vec<Value>> {
        let mut projected_row = Vec::new();

        let mut context = DefaultExpressionContext::new();

        // Set the value of the current row to the context variable.
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
                // Map edge type name to the edge value for GO queries like YIELD friend.name
                if let Value::Edge(ref edge_val) = row[edge_idx] {
                    context.set_variable(edge_val.edge_type().to_string(), row[edge_idx].clone());
                }
            }
        }

        // Handle table.column format: create table map variables
        let mut table_maps: std::collections::HashMap<
            String,
            std::collections::HashMap<String, Value>,
        > = std::collections::HashMap::new();
        for (i, col_name) in col_names.iter().enumerate() {
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
            context.set_variable(table, Value::map(map));
        }

        // Evaluate each projected column.
        for column in &self.columns {
            // Extract the Expression from the ContextualExpression.
            let expr = match column.expression.expression() {
                Some(meta) => meta.inner().clone(),
                None => continue,
            };

            match ExpressionEvaluator::evaluate(&expr, &mut context) {
                Ok(value) => projected_row.push(value),
                Err(e) => {
                    return Err(DBError::expression(format!(
                        "Failed to evaluate projection expression '{}': {}",
                        column.name, e
                    )));
                }
            }
        }

        Ok(projected_row)
    }

    /// Processing data set projections
    ///
    /// Choose the processing method based on the amount of data:
    /// The amount of data is less than single_thread_limit: The processing is done using a single thread.
    /// Large amount of data: Parallel processing using Rayon technology
    fn project_dataset(&self, dataset: crate::query::DataSet) -> DBResult<crate::query::DataSet> {
        let mut result_dataset = crate::query::DataSet::new();

        // Set new column names
        result_dataset.col_names = self.columns.iter().map(|c| c.name.clone()).collect();

        let total_size = dataset.rows.len();

        // Determine whether to use parallel computing based on the parallel configuration.
        if !self.parallel_config.should_use_parallel(total_size) {
            // If the amount of data is small or parallel processing is disabled, single-threaded processing should be used.
            for row in dataset.rows {
                let projected_row = self.project_row(&row, &dataset.col_names)?;
                result_dataset.rows.push(projected_row);
            }
        } else {
            // The amount of data is large; therefore, rayon parallel processing is used for processing it.
            let batch_size = self.parallel_config.calculate_batch_size(total_size);
            let columns = self.columns.clone();
            let col_names = dataset.col_names.clone();

            // Use `par_chunks` from `rayon` for parallel processing.
            let projected_rows: Vec<Vec<Value>> = dataset
                .rows
                .par_chunks(batch_size)
                .flat_map(|chunk| {
                    chunk
                        .iter()
                        .filter_map(|row| {
                            let mut context = DefaultExpressionContext::new();

                            // Set the value of the current row to the context variable.
                            for (i, col_name) in col_names.iter().enumerate() {
                                if i < row.len() {
                                    context.set_variable(col_name.clone(), row[i].clone());
                                }
                            }

                            // Map GO query special variables: $$ -> dst, $^ -> src, target -> dst, edge -> edge
                            if let Some(dst_idx) = col_names.iter().position(|c| c == "dst") {
                                if dst_idx < row.len() {
                                    context.set_variable("$$".to_string(), row[dst_idx].clone());
                                    context
                                        .set_variable("target".to_string(), row[dst_idx].clone());
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
                                    if let Value::Edge(ref edge_val) = row[edge_idx] {
                                        context.set_variable(
                                            edge_val.edge_type().to_string(),
                                            row[edge_idx].clone(),
                                        );
                                    }
                                }
                            }

                            // Handle table.column format: create table map variables
                            let mut table_maps: std::collections::HashMap<
                                String,
                                std::collections::HashMap<String, Value>,
                            > = std::collections::HashMap::new();
                            for (i, col_name) in col_names.iter().enumerate() {
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
                                context.set_variable(table, Value::map(map));
                            }

                            // Evaluate each projected column.
                            let mut projected_row = Vec::new();
                            for column in &columns {
                                // Extract the Expression from the ContextualExpression.
                                let expr = match column.expression.expression() {
                                    Some(meta) => meta.inner().clone(),
                                    None => return None,
                                };

                                match ExpressionEvaluator::evaluate(&expr, &mut context) {
                                    Ok(value) => projected_row.push(value),
                                    Err(_) => return None, // Skip the rows where the evaluation failed.
                                }
                            }
                            Some(projected_row)
                        })
                        .collect::<Vec<_>>()
                })
                .collect();

            result_dataset.rows = projected_rows;
        }

        Ok(result_dataset)
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for ProjectExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for ProjectExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else {
            ExecutionResult::DataSet(crate::query::DataSet::new())
        };

        if self.columns.is_empty() {
            return Ok(input_result);
        }

        let projected_result = match input_result {
            ExecutionResult::DataSet(dataset) => {
                let projected_dataset = self.project_dataset(dataset)?;
                ExecutionResult::DataSet(projected_dataset)
            }
            ExecutionResult::Success => ExecutionResult::Success,
            ExecutionResult::Empty => ExecutionResult::Empty,
            ExecutionResult::SpaceSwitched(summary) => ExecutionResult::SpaceSwitched(summary),
            ExecutionResult::Error(_) => input_result,
        };

        Ok(projected_result)
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
        self.base.is_open()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::Value;
    use crate::core::BinaryOperator;
    use crate::storage::MockStorage;

    #[test]
    fn test_simple_projection() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let expr = crate::core::Expression::Variable("col1".to_string());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_context.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_context.clone());

        let columns = vec![ProjectionColumn::new(
            "projected_col1".to_string(),
            ctx_expr,
        )];

        let executor = ProjectExecutor::new(1, storage, columns, expr_context);

        // Create a test dataset
        let mut input_dataset = crate::query::DataSet::new();
        input_dataset.col_names = vec!["col1".to_string(), "col2".to_string()];
        input_dataset.rows = vec![
            vec![Value::Int(1), Value::String("Alice".to_string())],
            vec![Value::Int(2), Value::String("Bob".to_string())],
            vec![Value::Int(3), Value::String("Charlie".to_string())],
        ];

        // Without setting the `inputExecutor`, directly call the `project_dataset` method to conduct the test.
        let projected_dataset = executor
            .project_dataset(input_dataset)
            .expect("Projection should succeed");

        // Verification results
        assert_eq!(projected_dataset.col_names, vec!["projected_col1"]);
        assert_eq!(projected_dataset.rows.len(), 3);
        assert_eq!(projected_dataset.rows[0], vec![Value::Int(1)]);
        assert_eq!(projected_dataset.rows[1], vec![Value::Int(2)]);
        assert_eq!(projected_dataset.rows[2], vec![Value::Int(3)]);
    }

    #[test]
    fn test_expression_projection() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let expr = crate::core::Expression::Binary {
            left: Box::new(crate::core::Expression::Variable("col1".to_string())),
            op: BinaryOperator::Add,
            right: Box::new(crate::core::Expression::Variable("col2".to_string())),
        };
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(expr);
        let expr_id = expr_context.register_expression(expr_meta);
        let ctx_expr = crate::core::types::ContextualExpression::new(expr_id, expr_context.clone());

        let columns = vec![ProjectionColumn::new("sum".to_string(), ctx_expr)];

        let executor = ProjectExecutor::new(1, storage, columns, expr_context);

        // Create a test dataset
        let mut input_dataset = crate::query::DataSet::new();
        input_dataset.col_names = vec!["col1".to_string(), "col2".to_string()];
        input_dataset.rows = vec![
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(2), Value::Int(20)],
            vec![Value::Int(3), Value::Int(30)],
        ];

        // Directly call the `project_dataset` method to conduct the test.
        let projected_dataset = executor
            .project_dataset(input_dataset)
            .expect("Projection should succeed");

        // Verification results
        assert_eq!(projected_dataset.col_names, vec!["sum"]);
        assert_eq!(projected_dataset.rows.len(), 3);
        assert_eq!(projected_dataset.rows[0], vec![Value::Int(11)]);
        assert_eq!(projected_dataset.rows[1], vec![Value::Int(22)]);
        assert_eq!(projected_dataset.rows[2], vec![Value::Int(33)]);
    }
}

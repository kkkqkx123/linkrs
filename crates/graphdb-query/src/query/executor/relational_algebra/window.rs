//! Window function executor module
//!
//! Implements the WindowExecutor which handles OVER clause processing.
//! Collects all input rows, partitions them, orders within partitions,
//! and computes window function values.

use std::cmp::Ordering;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::DBError;
use crate::core::value::NullType;
use crate::core::Value;
use crate::core::Expression;
use crate::query::executor::base::ExecutorEnum;
use crate::query::executor::base::InputExecutor;
use crate::query::executor::base::{BaseResultProcessor, ResultProcessor, ResultProcessorContext};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, ExecutorStats};
use crate::query::executor::expression::evaluation_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::ExpressionContext;
use crate::query::planning::plan::core::nodes::graph_operations::window_node::WindowFunctionSpec;
use crate::storage::StorageClient;

/// A row together with its partition key and sort key values, used for sorting
struct PartitionRow {
    values: Vec<Value>,
    partition_values: Vec<Value>,
    sort_values: Vec<Value>,
    original_index: usize,
}

/// WindowExecutor – processes window functions with OVER clause
pub struct WindowExecutor<S: StorageClient + Send + 'static> {
    base: BaseResultProcessor<S>,
    window_functions: Vec<WindowFunctionSpec>,
    input_executor: Option<Box<ExecutorEnum<S>>>,
}

impl<S: StorageClient> WindowExecutor<S> {
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        window_functions: Vec<WindowFunctionSpec>,
    ) -> Self {
        let base = BaseResultProcessor::new(
            id,
            "WindowExecutor".to_string(),
            "Performs window function operations on query results".to_string(),
            storage,
        );
        Self {
            base,
            window_functions,
            input_executor: None,
        }
    }

    fn process_input(&mut self) -> DBResult<crate::query::DataSet> {
        let input_result = if let Some(ref mut input_exec) = self.input_executor {
            input_exec.execute()?
        } else if let Some(input) = &self.base.input {
            input.clone()
        } else {
            return Err(DBError::query(
                "Window executor requires input executor".to_string(),
            ));
        };

        let mut dataset = match input_result {
            ExecutionResult::DataSet(dataset) => dataset,
            ExecutionResult::Empty
            | ExecutionResult::Success
            | ExecutionResult::SpaceSwitched(_) => crate::query::DataSet::new(),
            ExecutionResult::Error(msg) => return Err(DBError::query(msg)),
        };

        self.compute_window_functions(&mut dataset)?;
        Ok(dataset)
    }

    fn evaluate_expression(
        expr: &Expression,
        row: &[Value],
        col_names: &[String],
    ) -> Result<Value, DBError> {
        let mut ctx = DefaultExpressionContext::new();
        for (i, col_name) in col_names.iter().enumerate() {
            if i < row.len() {
                ctx.set_variable(col_name.clone(), row[i].clone());
            }
        }
        ExpressionEvaluator::evaluate(expr, &mut ctx)
            .map_err(|e| DBError::query(format!("Failed to evaluate expression: {}", e)))
    }

    fn compute_window_functions(
        &mut self,
        dataset: &mut crate::query::DataSet,
    ) -> DBResult<()> {
        let total_rows = dataset.rows.len();
        let col_names = dataset.col_names.clone();

        for wf_spec in &self.window_functions {
            let mut rows_with_keys: Vec<PartitionRow> = dataset
                .rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let partition_values = wf_spec
                        .partition_by
                        .iter()
                        .map(|expr| Self::evaluate_expression(expr, row, &col_names))
                        .collect::<Result<Vec<_>, DBError>>()?;
                    let sort_values = wf_spec
                        .order_by
                        .iter()
                        .map(|expr| Self::evaluate_expression(expr, row, &col_names))
                        .collect::<Result<Vec<_>, DBError>>()?;
                    Ok(PartitionRow {
                        values: row.clone(),
                        partition_values,
                        sort_values,
                        original_index: i,
                    })
                })
                .collect::<Result<Vec<_>, DBError>>()?;

            let sort_desc = wf_spec.order_desc.clone();
            rows_with_keys.sort_by(|a, b| {
                for (pa, pb) in a.partition_values.iter().zip(b.partition_values.iter()) {
                    match pa.partial_cmp(pb) {
                        Some(Ordering::Less) => return Ordering::Less,
                        Some(Ordering::Greater) => return Ordering::Greater,
                        _ => {}
                    }
                }
                for (j, (sa, sb)) in a.sort_values.iter().zip(b.sort_values.iter()).enumerate() {
                    let desc = sort_desc.get(j).copied().unwrap_or(false);
                    match sa.partial_cmp(sb) {
                        Some(Ordering::Less) => {
                            return if desc { Ordering::Greater } else { Ordering::Less }
                        }
                        Some(Ordering::Greater) => {
                            return if desc { Ordering::Less } else { Ordering::Greater }
                        }
                        _ => {}
                    }
                }
                Ordering::Equal
            });

            let mut partitions: Vec<Vec<&PartitionRow>> = Vec::new();
            let mut current_partition: Vec<&PartitionRow> = Vec::new();
            for row in &rows_with_keys {
                if current_partition.is_empty()
                    || current_partition
                        .last()
                        .map(|last| last.partition_values == row.partition_values)
                        .unwrap_or(false)
                {
                    current_partition.push(row);
                } else {
                    partitions.push(std::mem::take(&mut current_partition));
                    current_partition.push(row);
                }
            }
            if !current_partition.is_empty() {
                partitions.push(current_partition);
            }

            let mut result_values: Vec<Value> = vec![Value::Null(NullType::Null); total_rows];

            for partition in &partitions {
                let n = partition.len();
                for (pos, row_entry) in partition.iter().enumerate() {
                    let val = match wf_spec.name.as_str() {
                        "row_number" => Value::BigInt((pos + 1) as i64),
                        "rank" => Value::BigInt((pos + 1) as i64),
                        "dense_rank" => Value::BigInt((pos + 1) as i64),
                        "lead" => {
                            if !wf_spec.args.is_empty() {
                                let offset = if wf_spec.args.len() > 1 {
                                    if let Ok(offset_val) = Self::evaluate_expression(
                                        &wf_spec.args[1],
                                        &row_entry.values,
                                        &col_names,
                                    ) {
                                        match &offset_val {
                                            Value::Int(i) => *i as usize,
                                            Value::BigInt(i) => *i as usize,
                                            _ => 1,
                                        }
                                    } else {
                                        1
                                    }
                                } else {
                                    1
                                };
                                let lead_idx = pos + offset;
                                if lead_idx < n {
                                    if let Ok(v) = Self::evaluate_expression(
                                        &wf_spec.args[0],
                                        &partition[lead_idx].values,
                                        &col_names,
                                    ) {
                                        v
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
                        "lag" => {
                            if !wf_spec.args.is_empty() {
                                let offset = if wf_spec.args.len() > 1 {
                                    if let Ok(offset_val) = Self::evaluate_expression(
                                        &wf_spec.args[1],
                                        &row_entry.values,
                                        &col_names,
                                    ) {
                                        match &offset_val {
                                            Value::Int(i) => *i as usize,
                                            Value::BigInt(i) => *i as usize,
                                            _ => 1,
                                        }
                                    } else {
                                        1
                                    }
                                } else {
                                    1
                                };
                                if pos >= offset {
                                    let lag_idx = pos - offset;
                                    if let Ok(v) = Self::evaluate_expression(
                                        &wf_spec.args[0],
                                        &partition[lag_idx].values,
                                        &col_names,
                                    ) {
                                        v
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
                        "first_value" => {
                            if !wf_spec.args.is_empty() {
                                if let Ok(v) = Self::evaluate_expression(
                                    &wf_spec.args[0],
                                    &partition[0].values,
                                    &col_names,
                                ) {
                                    v
                                } else {
                                    Value::Null(NullType::Null)
                                }
                            } else {
                                Value::Null(NullType::Null)
                            }
                        }
                        "last_value" => {
                            if !wf_spec.args.is_empty() {
                                if let Ok(v) = Self::evaluate_expression(
                                    &wf_spec.args[0],
                                    &partition[n - 1].values,
                                    &col_names,
                                ) {
                                    v
                                } else {
                                    Value::Null(NullType::Null)
                                }
                            } else {
                                Value::Null(NullType::Null)
                            }
                        }
                        "nth_value" => {
                            if wf_spec.args.len() >= 2 {
                                if let Ok(n_val) = Self::evaluate_expression(
                                    &wf_spec.args[1],
                                    &row_entry.values,
                                    &col_names,
                                ) {
                                    let n = match &n_val {
                                        Value::Int(i) => *i as usize,
                                        Value::BigInt(i) => *i as usize,
                                        _ => 1,
                                    };
                                    if n > 0 {
                                        let idx = n - 1;
                                        if idx < n {
                                            if let Ok(v) = Self::evaluate_expression(
                                                &wf_spec.args[0],
                                                &partition[idx].values,
                                                &col_names,
                                            ) {
                                                v
                                            } else {
                                                Value::Null(NullType::Null)
                                            }
                                        } else {
                                            Value::Null(NullType::Null)
                                        }
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
                        "ntile" => {
                            if !wf_spec.args.is_empty() {
                                if let Ok(n_val) = Self::evaluate_expression(
                                    &wf_spec.args[0],
                                    &row_entry.values,
                                    &col_names,
                                ) {
                                    let num_buckets = match &n_val {
                                        Value::Int(i) => *i as usize,
                                        Value::BigInt(i) => *i as usize,
                                        _ => 1,
                                    };
                                    if num_buckets > 0 && n > 0 {
                                        let bucket_size = n / num_buckets;
                                        let remainder = n % num_buckets;
                                        let bucket = if pos < remainder * (bucket_size + 1) {
                                            pos / (bucket_size + 1)
                                        } else {
                                            (pos - remainder) / bucket_size
                                        };
                                        Value::BigInt((bucket + 1) as i64)
                                    } else {
                                        Value::BigInt(1)
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
                    result_values[row_entry.original_index] = val;
                }
            }

            for (i, val) in result_values.into_iter().enumerate() {
                dataset.rows[i].push(val);
            }
        }

        Ok(())
    }
}

impl<S: StorageClient + Send + 'static> ResultProcessor for WindowExecutor<S> {
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

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for WindowExecutor<S> {
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

    fn stats(&self) -> &ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient + Send + 'static> InputExecutor<S> for WindowExecutor<S> {
    fn set_input(&mut self, input: ExecutorEnum<S>) {
        self.input_executor = Some(Box::new(input));
    }

    fn get_input(&self) -> Option<&ExecutorEnum<S>> {
        self.input_executor.as_deref()
    }
}

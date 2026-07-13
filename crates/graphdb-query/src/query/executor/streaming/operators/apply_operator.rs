use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operator_base::OperatorBase;

#[derive(Debug)]
pub enum ApplyOperator {
    Apply {
        apply_expression: Expression,
    },
    PatternApply {
        pattern: Expression,
        all_rows: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        memory_tracker: MemoryTracker,
    },
}

impl ApplyOperator {
    pub fn from_spec(spec: &super::super::operator_spec::ApplySpec, budget: &MemoryBudget) -> Self {
        match spec {
            super::super::operator_spec::ApplySpec::Apply { apply_expression } => {
                Self::Apply {
                    apply_expression: apply_expression.clone(),
                }
            }
            super::super::operator_spec::ApplySpec::PatternApply { pattern } => Self::PatternApply {
                pattern: pattern.clone(),
                all_rows: Vec::new(),
                result_iter: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::PatternApply { memory_tracker, .. } => memory_tracker,
            Self::Apply { .. } => {
                panic!("memory_tracker called on variant without memory tracking")
            }
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Apply { .. } | Self::PatternApply { .. } => {
                left.open()?;
                _right.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Apply {
                apply_expression, ..
            } => {
                if let Some(chunk) = left.advance()? {
                    let col_names = chunk.col_names();
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let mut context = ValueRowContext::new(row.clone(), col_names.clone());
                        if let Ok(val) =
                            ExpressionEvaluator::evaluate(apply_expression, &mut context)
                        {
                            let mut new_row = row.clone();
                            new_row.push(val);
                            result_rows.push(new_row);
                        }
                    }

                    if !result_rows.is_empty() {
                        let result_col_names = {
                            let mut names = col_names.clone();
                            names.push("apply_result".to_string());
                            names
                        };
                        return Ok(Some(DataChunk::from_rows_with_col_names(
                            result_rows,
                            Some(result_col_names),
                        )));
                    }
                }
                Ok(None)
            }

            Self::PatternApply {
                pattern,
                all_rows,
                result_iter,
                memory_tracker,
                ..
            } => {
                if result_iter.is_none() {
                    while let Some(chunk) = left.advance()? {
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        let col_names = chunk.col_names();
                        for row in chunk.rows {
                            let mut ctx = ValueRowContext::new(row.clone(), col_names.clone());
                            match ExpressionEvaluator::evaluate(pattern, &mut ctx) {
                                Ok(val) => {
                                    let mut new_row = row.clone();
                                    new_row.push(val);
                                    all_rows.push(new_row);
                                }
                                Err(_) => {
                                    all_rows.push(row);
                                }
                            }
                        }
                    }
                    *result_iter = Some(std::mem::take(all_rows).into_iter());
                }

                if let Some(iter) = result_iter {
                    if let Some(row) = iter.next() {
                        let col_names = vec!["pattern_result".to_string()];
                        return Ok(Some(DataChunk::from_rows_with_col_names(
                            vec![row],
                            Some(col_names),
                        )));
                    }
                }

                Ok(None)
            }
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Apply { .. } | Self::PatternApply { .. } => {
                left.stop()?;
                right.stop()?;
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Apply { .. } => {
                let left_err = left.close().err();
                let right_err = right.close().err();
                close_result(left_err, right_err)
            }
            Self::PatternApply {
                all_rows,
                result_iter,
                memory_tracker,
                ..
            } => {
                memory_tracker.reset();
                let left_err = left.close().err();
                let right_err = right.close().err();
                all_rows.clear();
                *result_iter = None;
                close_result(left_err, right_err)
            }
        }
    }
}

fn close_result(left: Option<QueryError>, right: Option<QueryError>) -> Result<(), QueryError> {
    match (left, right) {
        (Some(e), _) => Err(e),
        (_, Some(e)) => Err(e),
        _ => Ok(()),
    }
}

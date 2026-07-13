use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::{NullType, Value};
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::join_helpers::evaluate_join_key;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::executor::streaming::operator_spec::{ApplyKind, ApplySpec};
use crate::query::executor::streaming::slot::{combine_layouts, SlotLayout};

#[derive(Debug)]
pub enum ApplyOperator {
    Apply {
        kind: ApplyKind,
        correlated_columns: Vec<String>,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
    PatternApply {
        key_expressions: Vec<crate::core::types::expr::Expression>,
        anti: bool,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
    RollUpApply {
        compare_columns: Vec<String>,
        collect_column: Option<String>,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
}

impl ApplyOperator {
    pub fn from_spec(spec: &ApplySpec, budget: &MemoryBudget) -> Self {
        match spec {
            ApplySpec::Apply {
                kind,
                correlated_columns,
            } => Self::Apply {
                kind: *kind,
                correlated_columns: correlated_columns.clone(),
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            ApplySpec::PatternApply {
                key_expressions,
                anti,
            } => Self::PatternApply {
                key_expressions: key_expressions.clone(),
                anti: *anti,
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            ApplySpec::RollUpApply {
                compare_columns,
                collect_column,
            } => Self::RollUpApply {
                compare_columns: compare_columns.clone(),
                collect_column: collect_column.clone(),
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::Apply { memory_tracker, .. }
            | Self::PatternApply { memory_tracker, .. }
            | Self::RollUpApply { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        left.open()?;
        right.open()?;
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Apply {
                kind,
                correlated_columns,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::from_names(&[])));
                loop {
                    let Some(left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    let output_layout = match kind {
                        ApplyKind::Semi | ApplyKind::Anti | ApplyKind::All => {
                            left_chunk.get_layout()
                        }
                        ApplyKind::Standard | ApplyKind::Single => {
                            Arc::new(combine_layouts(&left_chunk.get_layout(), &right_layout))
                        }
                    };
                    let mut output = Vec::new();
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        let matches = matching_rows(
                            &left_row,
                            &left_chunk.layout,
                            right_rows,
                            &right_layout,
                            correlated_columns,
                        )?;
                        match kind {
                            ApplyKind::Standard => {
                                for right_row in matches {
                                    let mut row = left_row.clone();
                                    row.extend_from_slice(right_row);
                                    output.push(row);
                                }
                            }
                            ApplyKind::Semi if !matches.is_empty() => output.push(left_row),
                            ApplyKind::Anti if matches.is_empty() => output.push(left_row),
                            ApplyKind::Single => match matches.as_slice() {
                                [] => {
                                    let mut row = left_row;
                                    row.extend(std::iter::repeat_n(
                                        Value::Null(NullType::Null),
                                        right_layout.len(),
                                    ));
                                    output.push(row);
                                }
                                [right_row] => {
                                    let mut row = left_row;
                                    row.extend_from_slice(right_row);
                                    output.push(row);
                                }
                                _ => {
                                    return Err(QueryError::execution(
                                        "Single Apply produced more than one matching row"
                                            .to_string(),
                                    ));
                                }
                            },
                            ApplyKind::All if matches.len() == right_rows.len() => {
                                output.push(left_row);
                            }
                            _ => {}
                        }
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(output, output_layout)));
                    }
                }
            }
            Self::PatternApply {
                key_expressions,
                anti,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::from_names(&[])));
                loop {
                    let Some(left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    let mut output = Vec::new();
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        let left_key = evaluate_join_key(
                            &left_row,
                            left_chunk.layout.clone(),
                            key_expressions,
                        )?;
                        let mut exists = false;
                        for right_row in right_rows {
                            let right_key = evaluate_join_key(
                                right_row,
                                right_layout.clone(),
                                key_expressions,
                            )?;
                            if keys_match(&left_key, &right_key) {
                                exists = true;
                                break;
                            }
                        }
                        if exists != *anti {
                            output.push(left_row);
                        }
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(output, left_chunk.layout)));
                    }
                }
            }
            Self::RollUpApply {
                compare_columns,
                collect_column,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let Some(left_chunk) = left.advance()? else {
                    return Ok(None);
                };
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::from_names(&[])));
                let collect_slot = collect_column
                    .as_deref()
                    .map(|column| {
                        right_layout.resolve(column).ok_or_else(|| {
                            QueryError::execution(format!(
                                "RollUpApply collect column not found: {column}"
                            ))
                        })
                    })
                    .transpose()?;
                let mut names = left_chunk.layout.names();
                names.push(
                    collect_column
                        .clone()
                        .unwrap_or_else(|| "rollup".to_string()),
                );
                let output_layout = Arc::new(SlotLayout::from_names(&names));
                let mut output = Vec::with_capacity(left_chunk.rows.len());
                for left_row in left_chunk.rows {
                    base.ensure_not_cancelled()?;
                    let matches = matching_rows(
                        &left_row,
                        &left_chunk.layout,
                        right_rows,
                        &right_layout,
                        compare_columns,
                    )?;
                    let values = matches
                        .into_iter()
                        .map(|row| {
                            collect_slot
                                .and_then(|slot| row.get(slot).cloned())
                                .unwrap_or_else(|| {
                                    Value::List(Box::new(crate::core::value::List {
                                        values: row.clone(),
                                    }))
                                })
                        })
                        .collect();
                    let mut row = left_row;
                    row.push(Value::List(Box::new(crate::core::value::List { values })));
                    output.push(row);
                }
                Ok(Some(DataChunk::new_with_layout(output, output_layout)))
            }
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        let left_error = left.stop().err();
        let right_error = right.stop().err();
        close_result(left_error, right_error)
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if !base.lifecycle.can_close() {
            return Ok(());
        }
        match self {
            Self::Apply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            }
            | Self::PatternApply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            }
            | Self::RollUpApply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            } => {
                right_rows.take();
                right_layout.take();
                memory_tracker.reset();
            }
        }
        let left_error = left.close().err();
        let right_error = right.close().err();
        base.lifecycle.mark_closed();
        close_result(left_error, right_error)
    }
}

fn materialize_right(
    base: &OperatorBase,
    right: &mut StreamingExecutor,
    rows: &mut Option<Vec<Vec<Value>>>,
    layout: &mut Option<Arc<SlotLayout>>,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    if rows.is_some() {
        return Ok(());
    }
    let mut materialized = Vec::new();
    while let Some(chunk) = right.advance()? {
        base.ensure_not_cancelled()?;
        if layout.is_none() {
            *layout = Some(chunk.get_layout());
        }
        for row in &chunk.rows {
            memory_tracker.try_reserve_row(row)?;
        }
        materialized.extend(chunk.rows);
    }
    *rows = Some(materialized);
    Ok(())
}

fn matching_rows<'a>(
    left_row: &[Value],
    left_layout: &SlotLayout,
    right_rows: &'a [Vec<Value>],
    right_layout: &SlotLayout,
    correlated_columns: &[String],
) -> Result<Vec<&'a Vec<Value>>, QueryError> {
    let mut slots = Vec::with_capacity(correlated_columns.len());
    for column in correlated_columns {
        let left_slot = left_layout.resolve(column).ok_or_else(|| {
            QueryError::execution(format!("Apply left correlation column not found: {column}"))
        })?;
        let right_slot = right_layout.resolve(column).ok_or_else(|| {
            QueryError::execution(format!(
                "Apply right correlation column not found: {column}"
            ))
        })?;
        slots.push((left_slot, right_slot));
    }
    Ok(right_rows
        .iter()
        .filter(|right_row| {
            slots.iter().all(|(left_slot, right_slot)| {
                match (left_row.get(*left_slot), right_row.get(*right_slot)) {
                    (Some(Value::Null(_)), _) | (_, Some(Value::Null(_))) => false,
                    (Some(left), Some(right)) => left == right,
                    _ => false,
                }
            })
        })
        .collect())
}

fn keys_match(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            !matches!(left, Value::Null(_)) && !matches!(right, Value::Null(_)) && left == right
        })
}

fn close_result(left: Option<QueryError>, right: Option<QueryError>) -> Result<(), QueryError> {
    match (left, right) {
        (Some(error), _) | (None, Some(error)) => Err(error),
        (None, None) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::builder::StreamingExecutorBuilder;

    fn execute_apply(
        spec: ApplySpec,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        let left = StreamingExecutorBuilder::build_simple_scan_with_col_names(
            left_rows,
            vec!["id".to_string()],
        )?;
        let right = StreamingExecutorBuilder::build_simple_scan_with_col_names(
            right_rows,
            vec!["id".to_string()],
        )?;
        let budget = MemoryBudget::default_budget();
        let operator = ApplyOperator::from_spec(&spec, &budget);
        let mut executor = StreamingExecutor::Apply(
            OperatorBase::new(3),
            Box::new(left),
            Box::new(right),
            operator,
        );
        executor.open()?;
        let mut rows = Vec::new();
        while let Some(chunk) = executor.advance()? {
            rows.extend(chunk.rows);
        }
        executor.close()?;
        Ok(rows)
    }

    #[test]
    fn semi_and_anti_apply_consume_the_right_input() {
        let semi = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Semi,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec![vec![Value::Int(2)]],
        )
        .expect("semi apply should execute");
        assert_eq!(semi, vec![vec![Value::Int(2)]]);

        let anti = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Anti,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec![vec![Value::Int(2)]],
        )
        .expect("anti apply should execute");
        assert_eq!(anti, vec![vec![Value::Int(1)]]);
    }

    #[test]
    fn single_apply_rejects_multiple_matches() {
        let result = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Single,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)]],
            vec![vec![Value::Int(1)], vec![Value::Int(1)]],
        );
        assert!(result.is_err());
    }
}

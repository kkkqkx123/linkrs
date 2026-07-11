use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::FullOuterJoinPhase;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::executor::ValueRowContext;
use crate::query::executor::streaming::operator_base::OperatorBase;
use crate::query::executor::streaming::slot::{combine_layouts, SlotLayout};

fn build_combined_names(
    left_col_names: &[String],
    right_col_names: &[String],
    fallback_right_width: usize,
) -> Vec<String> {
    let mut names = left_col_names.to_vec();
    if !right_col_names.is_empty() {
        names.extend_from_slice(right_col_names);
    } else {
        for i in 0..fallback_right_width {
            names.push(format!("right_{}", i));
        }
    }
    names
}

fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[Expression],
) -> Result<Vec<Value>, QueryError> {
    if key_expressions.is_empty() {
        return Ok(Vec::new());
    }

    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let mut context = ValueRowContext::new(row.to_vec(), col_names.to_vec());
        let value = ExpressionEvaluator::evaluate(expr, &mut context)
            .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))?;
        key.push(value);
    }
    Ok(key)
}

#[derive(Debug)]
pub enum JoinOperator {
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side_hash: HashMap<Vec<Value>, Vec<Vec<Value>>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    InnerJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    RightJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        matched_right_indices: HashSet<usize>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        phase: FullOuterJoinPhase,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    CrossJoin {
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    SemiJoin {
        join_condition: Option<Expression>,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
}

impl JoinOperator {
    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::HashJoin { memory_tracker, .. }
            | Self::HashLeftJoin { memory_tracker, .. }
            | Self::NestedLoopJoin { memory_tracker, .. }
            | Self::InnerJoin { memory_tracker, .. }
            | Self::LeftJoin { memory_tracker, .. }
            | Self::RightJoin { memory_tracker, .. }
            | Self::FullOuterJoin { memory_tracker, .. }
            | Self::CrossJoin { memory_tracker, .. }
            | Self::SemiJoin { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::HashJoin { .. }
            | Self::HashLeftJoin { .. }
            | Self::NestedLoopJoin { .. }
            | Self::InnerJoin { .. }
            | Self::LeftJoin { .. }
            | Self::RightJoin { .. }
            | Self::FullOuterJoin { .. }
            | Self::CrossJoin { .. }
            | Self::SemiJoin { .. } => {
                left.open()?;
                right.open()?;
                base.opened = true;
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    while let Some(chunk) = right.advance()? {
                        let col_names = chunk.col_names();
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            let hash_key =
                                evaluate_join_key(&row, &col_names, hash_keys)?;
                            build_side_hash
                                .entry(hash_key)
                                .or_default()
                                .push(row.clone());
                            all_right_rows.push(row);
                        }
                    }
                    *left_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        let probe_key =
                            evaluate_join_key(left_row, &left_col_names, probe_keys)?;
                        let matching_right_rows = build_side_hash.get(&probe_key);

                        if let Some(right_rows) = matching_right_rows {
                            for right_row in right_rows {
                                let condition_satisfied =
                                    if let Some(condition) = join_condition {
                                        let mut combined_row = left_row.clone();
                                        combined_row.extend(right_row.clone());
                                        let combined_names = build_combined_names(
                                            &left_col_names,
                                            right_col_names,
                                            right_row.len(),
                                        );
                                        let mut context =
                                            ValueRowContext::new(combined_row, combined_names);
                                        match ExpressionEvaluator::evaluate(condition, &mut context)
                                        {
                                            Ok(Value::Bool(b)) => b,
                                            _ => false,
                                        }
                                    } else {
                                        true
                                    };

                                if condition_satisfied {
                                    let mut joined_row = left_row.clone();
                                    joined_row.extend(right_row.clone());
                                    result_rows.push(joined_row);
                                }
                            }
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        let right_layout = Arc::new(SlotLayout::from_names(
                            &build_combined_names(
                                &[],
                                right_col_names,
                                all_right_rows
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0),
                            ),
                        ));
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side_hash,
                all_right_rows,
                left_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    while let Some(chunk) = right.advance()? {
                        let col_names = chunk.col_names();
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            let hash_key =
                                evaluate_join_key(&row, &col_names, hash_keys)?;
                            build_side_hash
                                .entry(hash_key)
                                .or_default()
                                .push(row.clone());
                            all_right_rows.push(row);
                        }
                    }
                    *left_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        let probe_key =
                            evaluate_join_key(left_row, &left_col_names, probe_keys)?;
                        let matching_right_rows = build_side_hash.get(&probe_key);

                        if let Some(right_rows) = matching_right_rows {
                            for right_row in right_rows {
                                let condition_satisfied =
                                    if let Some(condition) = join_condition {
                                        let mut combined_row = left_row.clone();
                                        combined_row.extend(right_row.clone());
                                        let combined_names = build_combined_names(
                                            &left_col_names,
                                            right_col_names,
                                            right_row.len(),
                                        );
                                        let mut context =
                                            ValueRowContext::new(combined_row, combined_names);
                                        match ExpressionEvaluator::evaluate(condition, &mut context)
                                        {
                                            Ok(value) => match value {
                                                Value::Bool(b) => b,
                                                Value::Null(_) => false,
                                                _ => true,
                                            },
                                            Err(e) => {
                                                return Err(QueryError::execution(format!(
                                                    "HashLeftJoin condition evaluation failed: {}",
                                                    e
                                                )));
                                            }
                                        }
                                    } else {
                                        true
                                    };

                                if condition_satisfied {
                                    let mut joined_row = left_row.clone();
                                    joined_row.extend(right_row.clone());
                                    result_rows.push(joined_row);
                                }
                            }
                        } else {
                            let mut unmatched_row = left_row.clone();
                            for _ in
                                0..all_right_rows.first().map(|r| r.len()).unwrap_or(0)
                            {
                                unmatched_row
                                    .push(Value::Null(crate::core::value::NullType::Null));
                            }
                            result_rows.push(unmatched_row);
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        let right_layout = if right_col_names.is_empty() {
                            Arc::new(SlotLayout::from_names(
                                &all_right_rows
                                    .first()
                                    .map(|r| {
                                        (0..r.len())
                                            .map(|i| format!("right_{}", i))
                                            .collect::<Vec<_>>()
                                    })
                                    .unwrap_or_default(),
                            ))
                        } else {
                            Arc::new(SlotLayout::from_names(right_col_names))
                        };
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::NestedLoopJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    let mut captured_right_names = Vec::new();
                    while let Some(chunk) = right.advance()? {
                        if captured_right_names.is_empty() {
                            captured_right_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            build_side_tuples.push(row);
                        }
                    }
                    *right_col_names = captured_right_names;
                    *left_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        for right_row in build_side_tuples.iter() {
                            let condition_satisfied =
                                if let Some(condition) = join_condition {
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let combined_names = build_combined_names(
                                        &left_col_names,
                                        right_col_names,
                                        right_row.len(),
                                    );
                                    let mut context =
                                        ValueRowContext::new(combined_row, combined_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                                        Ok(value) => match value {
                                            Value::Bool(b) => b,
                                            Value::Null(_) => false,
                                            _ => true,
                                        },
                                        Err(e) => {
                                            return Err(QueryError::execution(format!(
                                                "NestedLoopJoin condition evaluation failed: {}",
                                                e
                                            )));
                                        }
                                    }
                                } else {
                                    true
                                };

                            if condition_satisfied {
                                let mut joined_row = left_row.clone();
                                joined_row.extend(right_row.clone());
                                result_rows.push(joined_row);
                            }
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        let right_layout = Arc::new(SlotLayout::from_names(
                            &build_combined_names(
                                &[],
                                right_col_names,
                                build_side_tuples
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0),
                            ),
                        ));
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::InnerJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    let mut captured_right_names = Vec::new();
                    while let Some(chunk) = right.advance()? {
                        if captured_right_names.is_empty() {
                            captured_right_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            build_side_tuples.push(row);
                        }
                    }
                    *right_col_names = captured_right_names;
                    *left_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        for right_row in build_side_tuples.iter() {
                            let condition_satisfied =
                                if let Some(condition) = join_condition {
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let combined_names = build_combined_names(
                                        &left_col_names,
                                        right_col_names,
                                        right_row.len(),
                                    );
                                    let mut context =
                                        ValueRowContext::new(combined_row, combined_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                                        Ok(Value::Bool(b)) => b,
                                        _ => false,
                                    }
                                } else {
                                    true
                                };

                            if condition_satisfied {
                                let mut joined_row = left_row.clone();
                                joined_row.extend(right_row.clone());
                                result_rows.push(joined_row);
                            }
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        let right_layout = Arc::new(SlotLayout::from_names(
                            &build_combined_names(
                                &[],
                                right_col_names,
                                build_side_tuples
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0),
                            ),
                        ));
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::LeftJoin {
                join_condition,
                build_side_tuples,
                left_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    let mut captured_right_names = Vec::new();
                    while let Some(chunk) = right.advance()? {
                        if captured_right_names.is_empty() {
                            captured_right_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            build_side_tuples.push(row);
                        }
                    }
                    *right_col_names = captured_right_names;
                    *left_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        let mut matched = false;
                        for right_row in build_side_tuples.iter() {
                            let condition_satisfied =
                                if let Some(condition) = join_condition {
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let combined_names = build_combined_names(
                                        &left_col_names,
                                        right_col_names,
                                        right_row.len(),
                                    );
                                    let mut context =
                                        ValueRowContext::new(combined_row, combined_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                                        Ok(Value::Bool(b)) => b,
                                        _ => false,
                                    }
                                } else {
                                    true
                                };

                            if condition_satisfied {
                                matched = true;
                                let mut joined_row = left_row.clone();
                                joined_row.extend(right_row.clone());
                                result_rows.push(joined_row);
                            }
                        }

                        if !matched {
                            let mut unmatched_row = left_row.clone();
                            for _ in 0..build_side_tuples
                                .first()
                                .map(|r| r.len())
                                .unwrap_or(0)
                            {
                                unmatched_row
                                    .push(Value::Null(crate::core::value::NullType::Null));
                            }
                            result_rows.push(unmatched_row);
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        let right_layout = Arc::new(SlotLayout::from_names(
                            &build_combined_names(
                                &[],
                                right_col_names,
                                build_side_tuples
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0),
                            ),
                        ));
                        let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::RightJoin {
                join_condition,
                build_side_tuples,
                right_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*right_consumed {
                    let mut captured_left_names = Vec::new();
                    while let Some(chunk) = left.advance()? {
                        if captured_left_names.is_empty() {
                            captured_left_names = chunk.col_names();
                        }
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            build_side_tuples.push(row);
                        }
                    }
                    *right_col_names = captured_left_names;
                    *right_consumed = true;
                }

                if let Some(right_chunk) = right.advance()? {
                    let right_cols = right_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for right_row in &right_chunk.rows {
                        let mut matched = false;
                        for left_row in build_side_tuples.iter() {
                            let condition_satisfied =
                                if let Some(condition) = join_condition {
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let combined_names = build_combined_names(
                                        right_col_names,
                                        &right_cols,
                                        left_row.len(),
                                    );
                                    let mut context =
                                        ValueRowContext::new(combined_row, combined_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                                        Ok(Value::Bool(b)) => b,
                                        _ => false,
                                    }
                                } else {
                                    true
                                };

                            if condition_satisfied {
                                matched = true;
                                let mut joined_row = left_row.clone();
                                joined_row.extend(right_row.clone());
                                result_rows.push(joined_row);
                            }
                        }

                        if !matched {
                            let mut unmatched_row = Vec::new();
                            for _ in 0..build_side_tuples
                                .first()
                                .map(|r| r.len())
                                .unwrap_or(0)
                            {
                                unmatched_row
                                    .push(Value::Null(crate::core::value::NullType::Null));
                            }
                            unmatched_row.extend(right_row.clone());
                            result_rows.push(unmatched_row);
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout =
                            if let Some(first_left) = build_side_tuples.first() {
                                Arc::new(SlotLayout::from_names(&build_combined_names(
                                    right_col_names,
                                    &[],
                                    first_left.len(),
                                )))
                            } else {
                                Arc::new(SlotLayout::from_names(&[]))
                            };
                        let right_layout =
                            Arc::new(SlotLayout::from_names(&right_cols));
                        let layout =
                            Arc::new(combine_layouts(&left_layout, &right_layout));
                        Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::FullOuterJoin {
                join_condition,
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                memory_tracker,
                right_col_names,
                ..
            } => loop {
                match phase {
                    FullOuterJoinPhase::BuildingRight => {
                        let mut captured_right_names = Vec::new();
                        while let Some(chunk) = left.advance()? {
                            for row in &chunk.rows {
                                memory_tracker.try_reserve_row(row)?;
                            }
                            left_rows.extend(chunk.rows);
                        }
                        while let Some(chunk) = right.advance()? {
                            if captured_right_names.is_empty() {
                                captured_right_names = chunk.col_names();
                            }
                            for row in &chunk.rows {
                                memory_tracker.try_reserve_row(row)?;
                            }
                            right_rows.extend(chunk.rows);
                        }
                        *right_col_names = captured_right_names;
                        *phase = FullOuterJoinPhase::ProbeLeft;
                    }

                    FullOuterJoinPhase::ProbeLeft => {
                        let right_col_count =
                            right_rows.first().map(|r| r.len()).unwrap_or(0);
                        let mut all_results = Vec::new();

                        for left_row in left_rows.iter() {
                            let mut matched = false;
                            for (right_idx, right_row) in right_rows.iter().enumerate() {
                                let condition_satisfied =
                                    if let Some(condition) = join_condition {
                                        let left_col_names: Vec<String> = (0..left_row.len())
                                            .map(|i| format!("col_{}", i))
                                            .collect();
                                        let mut combined_row = left_row.clone();
                                        combined_row.extend(right_row.clone());
                                        let combined_names = build_combined_names(
                                            &left_col_names,
                                            right_col_names,
                                            right_row.len(),
                                        );
                                        let mut context =
                                            ValueRowContext::new(combined_row, combined_names);
                                        match ExpressionEvaluator::evaluate(
                                            condition, &mut context,
                                        ) {
                                            Ok(Value::Bool(b)) => b,
                                            _ => false,
                                        }
                                    } else {
                                        true
                                    };

                                if condition_satisfied {
                                    matched = true;
                                    matched_right_indices.insert(right_idx);
                                    let mut joined_row = left_row.clone();
                                    joined_row.extend(right_row.clone());
                                    all_results.push(joined_row);
                                }
                            }

                            if !matched {
                                let mut unmatched_row = left_row.clone();
                                for _ in 0..right_col_count {
                                    unmatched_row
                                        .push(Value::Null(crate::core::value::NullType::Null));
                                }
                                all_results.push(unmatched_row);
                            }
                        }

                        *phase = FullOuterJoinPhase::EmitUnmatchedRight;
                        if !all_results.is_empty() {
                            let left_layout = Arc::new(SlotLayout::from_names(
                                &(0..left_rows
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0))
                                .map(|i| format!("col_{}", i))
                                .collect::<Vec<_>>(),
                            ));
                            let right_layout = Arc::new(SlotLayout::from_names(
                                &build_combined_names(
                                    &[],
                                    right_col_names,
                                    right_col_count,
                                ),
                            ));
                            let layout =
                                Arc::new(combine_layouts(&left_layout, &right_layout));
                            let rows: Vec<Vec<Value>> =
                                all_results.into_iter().collect();
                            if !rows.is_empty() {
                                *result_iter = Some(rows.into_iter());
                                return Ok(Some(DataChunk::new_with_layout(
                                    result_iter
                                        .as_mut()
                                        .unwrap()
                                        .collect::<Vec<_>>(),
                                    layout,
                                )));
                            }
                        }
                    }

                    FullOuterJoinPhase::EmitUnmatchedRight => {
                        if let Some(iter) = result_iter {
                            let rows: Vec<Vec<Value>> = iter.collect();
                            if !rows.is_empty() {
                                let left_layout = Arc::new(SlotLayout::from_names(
                                    &(0..left_rows
                                        .first()
                                        .map(|r| r.len())
                                        .unwrap_or(0))
                                    .map(|i| format!("col_{}", i))
                                    .collect::<Vec<_>>(),
                                ));
                                let right_layout = Arc::new(SlotLayout::from_names(
                                    &build_combined_names(
                                        &[],
                                        right_col_names,
                                        right_rows
                                            .first()
                                            .map(|r| r.len())
                                            .unwrap_or(0),
                                    ),
                                ));
                                let layout =
                                    Arc::new(combine_layouts(&left_layout, &right_layout));
                                return Ok(Some(DataChunk::new_with_layout(rows, layout)));
                            }
                            *result_iter = None;
                        }

                        let left_col_count = left_rows
                            .first()
                            .map(|r| r.len())
                            .unwrap_or(0);
                        let mut unmatched = Vec::new();
                        for (right_idx, right_row) in right_rows.iter().enumerate() {
                            if !matched_right_indices.contains(&right_idx) {
                                let mut row = Vec::new();
                                for _ in 0..left_col_count {
                                    row.push(Value::Null(
                                        crate::core::value::NullType::Null,
                                    ));
                                }
                                row.extend(right_row.clone());
                                unmatched.push(row);
                            }
                        }

                        if unmatched.is_empty() {
                            return Ok(None);
                        }
                        let left_layout = Arc::new(SlotLayout::from_names(
                            &(0..left_col_count)
                                .map(|i| format!("col_{}", i))
                                .collect::<Vec<_>>(),
                        ));
                        let right_layout = Arc::new(SlotLayout::from_names(
                            &build_combined_names(
                                &[],
                                right_col_names,
                                right_rows
                                    .first()
                                    .map(|r| r.len())
                                    .unwrap_or(0),
                            ),
                        ));
                        let layout =
                            Arc::new(combine_layouts(&left_layout, &right_layout));
                        return Ok(Some(DataChunk::new_with_layout(unmatched, layout)));
                    }
                }
            },

            Self::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                memory_tracker,
                right_col_names,
                ..
            } => {
                if !*left_consumed {
                    while let Some(chunk) = left.advance()? {
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        all_left_rows.extend(chunk.rows);
                    }
                    *left_consumed = true;
                }

                if !*right_consumed {
                    let mut captured_right_names = Vec::new();
                    while let Some(chunk) = right.advance()? {
                        if captured_right_names.is_empty() {
                            captured_right_names = chunk.col_names();
                        }
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        all_right_rows.extend(chunk.rows);
                    }
                    *right_col_names = captured_right_names;
                    *right_consumed = true;
                }

                if all_left_rows.is_empty() || all_right_rows.is_empty() {
                    return Ok(None);
                }

                let mut result_rows = Vec::new();
                for left_row in all_left_rows.iter() {
                    for right_row in all_right_rows.iter() {
                        let mut joined_row = left_row.clone();
                        joined_row.extend(right_row.clone());
                        result_rows.push(joined_row);
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    let left_layout = Arc::new(SlotLayout::from_names(
                        &(0..all_left_rows
                            .first()
                            .map(|r| r.len())
                            .unwrap_or(0))
                        .map(|i| format!("col_{}", i))
                        .collect::<Vec<_>>(),
                    ));
                    let right_layout = Arc::new(SlotLayout::from_names(
                        &build_combined_names(
                            &[],
                            right_col_names,
                            all_right_rows
                                .first()
                                .map(|r| r.len())
                                .unwrap_or(0),
                        ),
                    ));
                    let layout = Arc::new(combine_layouts(&left_layout, &right_layout));
                    Ok(Some(DataChunk::new_with_layout(result_rows, layout)))
                }
            }

            Self::SemiJoin {
                join_condition,
                right_rows,
                right_consumed,
                memory_tracker,
                ..
            } => {
                if !*right_consumed {
                    while let Some(chunk) = right.advance()? {
                        for row in chunk.rows {
                            memory_tracker.try_reserve_row(&row)?;
                            right_rows.push(row);
                        }
                    }
                    *right_consumed = true;
                }

                if let Some(left_chunk) = left.advance()? {
                    let left_col_names = left_chunk.col_names();
                    let mut result_rows = Vec::new();

                    for left_row in &left_chunk.rows {
                        for right_row in right_rows.iter() {
                            let condition_satisfied =
                                if let Some(condition) = join_condition {
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let mut combined_col_names = left_col_names.clone();
                                    for i in 0..right_row.len() {
                                        combined_col_names.push(format!("right_{}", i));
                                    }
                                    let mut context =
                                        ValueRowContext::new(combined_row, combined_col_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
                                        Ok(Value::Bool(b)) => b,
                                        _ => false,
                                    }
                                } else {
                                    true
                                };

                            if condition_satisfied {
                                result_rows.push(left_row.clone());
                                break;
                            }
                        }
                    }

                    if result_rows.is_empty() {
                        Ok(None)
                    } else {
                        let left_layout = left_chunk.get_or_create_layout();
                        Ok(Some(DataChunk::new_with_layout(result_rows, left_layout)))
                    }
                } else {
                    Ok(None)
                }
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
            Self::HashJoin { .. }
            | Self::HashLeftJoin { .. }
            | Self::NestedLoopJoin { .. }
            | Self::InnerJoin { .. }
            | Self::LeftJoin { .. }
            | Self::RightJoin { .. }
            | Self::FullOuterJoin { .. }
            | Self::CrossJoin { .. }
            | Self::SemiJoin { .. } => {
                left.stop()?;
                right.stop()
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::HashJoin {
                build_side_hash,
                all_right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(all_right_rows);
                    memory_tracker.release(mem);
                    build_side_hash.clear();
                    all_right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::HashLeftJoin {
                build_side_hash,
                all_right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(all_right_rows);
                    memory_tracker.release(mem);
                    build_side_hash.clear();
                    all_right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::NestedLoopJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                    memory_tracker.release(mem);
                    build_side_tuples.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::InnerJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                    memory_tracker.release(mem);
                    build_side_tuples.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::LeftJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                    memory_tracker.release(mem);
                    build_side_tuples.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::RightJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(build_side_tuples);
                    memory_tracker.release(mem);
                    build_side_tuples.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::FullOuterJoin {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem_left = MemoryBudget::estimate_rows_memory(left_rows);
                    let mem_right = MemoryBudget::estimate_rows_memory(right_rows);
                    memory_tracker.release(mem_left + mem_right);
                    left_rows.clear();
                    right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::CrossJoin {
                all_left_rows,
                all_right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem_left = MemoryBudget::estimate_rows_memory(all_left_rows);
                    let mem_right = MemoryBudget::estimate_rows_memory(all_right_rows);
                    memory_tracker.release(mem_left + mem_right);
                    all_left_rows.clear();
                    all_right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::SemiJoin {
                right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = MemoryBudget::estimate_rows_memory(right_rows);
                    memory_tracker.release(mem);
                    right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
        }
    }
}

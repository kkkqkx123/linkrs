//! Binary operators: HashJoin, NestedLoopJoin

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::{StreamingExecutor, ValueRowContext};

// ============ HashJoin ============

pub fn open_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_hashjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            build_side_hash,
            all_right_rows,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        all_right_rows.push(row.clone());
                        let key = format!("{:?}", row);
                        build_side_hash.entry(key).or_default().push(row);
                    }
                }
                *left_consumed = true;
            }

            if all_right_rows.is_empty() {
                return Ok(None);
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                if join_condition.is_none() {
                    // Cartesian product: match every left row with every right row
                    for left_row in &left_chunk.rows {
                        for right_row in all_right_rows.iter() {
                            let mut joined_row = left_row.clone();
                            joined_row.extend(right_row.clone());
                            result_rows.push(joined_row);
                        }
                    }
                } else {
                    // Hash join: match left rows via hash key, then verify condition
                    let condition = join_condition.as_ref().unwrap();
                    for left_row in &left_chunk.rows {
                        let probe_key = format!("{:?}", left_row);
                        if let Some(matching_rows) = build_side_hash.get(&probe_key) {
                            for right_row in matching_rows {
                                let mut combined_row = left_row.clone();
                                combined_row.extend(right_row.clone());

                                let mut combined_col_names = left_col_names.clone();
                                for i in 0..right_row.len() {
                                    combined_col_names.push(format!("right_{}", i));
                                }

                                let mut context =
                                    ValueRowContext::new(combined_row, combined_col_names);

                                let condition_satisfied = match ExpressionEvaluator::evaluate(condition, &mut context) {
                                    Ok(Value::Bool(b)) => b,
                                    _ => false,
                                };

                                if condition_satisfied {
                                    let mut joined_row = left_row.clone();
                                    joined_row.extend(right_row.clone());
                                    result_rows.push(joined_row);
                                }
                            }
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_hashjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::HashJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn reset_hashjoin(executor: &mut StreamingExecutor) {
    if let StreamingExecutor::HashJoin { build_side_hash, all_right_rows, left_consumed, .. } = executor {
        build_side_hash.clear();
        all_right_rows.clear();
        *left_consumed = false;
    }
}

// ============ NestedLoopJoin ============

pub fn open_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                // Build right side - collect all rows
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }
                *left_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    for right_row in build_side_tuples.iter() {
                        // Always evaluate condition for nested loop join
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());

                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }

                            let mut context =
                                ValueRowContext::new(combined_row, combined_col_names);
                            match ExpressionEvaluator::evaluate(condition, &mut context) {
                                Ok(value) => match value {
                                    Value::Bool(b) => b,
                                    Value::Null(_) => false,
                                    _ => true,
                                },
                                Err(_) => false,
                            }
                        } else {
                            // Cartesian product
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_nestedloopjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::NestedLoopJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ InnerJoin (standard) ============

pub fn open_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_innerjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                // Build right side
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }
                *left_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    for right_row in build_side_tuples.iter() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }
                            let mut context = ValueRowContext::new(combined_row, combined_col_names);
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_innerjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::InnerJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ LeftJoin ============

pub fn open_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_leftjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            left_consumed,
            ..
        } => {
            if !*left_consumed {
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }
                *left_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    let mut matched = false;
                    for right_row in build_side_tuples.iter() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }
                            let mut context = ValueRowContext::new(combined_row, combined_col_names);
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

                    // If no match, emit left row with NULLs for right columns
                    if !matched {
                        let mut unmatched_row = left_row.clone();
                        for _ in 0..build_side_tuples.get(0).map(|r| r.len()).unwrap_or(0) {
                            unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        result_rows.push(unmatched_row);
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_leftjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::LeftJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ RightJoin ============

pub fn open_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_rightjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            join_condition,
            build_side_tuples,
            right_consumed,
            ..
        } => {
            if !*right_consumed {
                while let Some(chunk) = left.next()? {
                    for row in chunk.rows {
                        build_side_tuples.push(row);
                    }
                }
                *right_consumed = true;
            }

            if let Some(right_chunk) = right.next()? {
                let right_col_names = right_chunk.col_names();
                let mut result_rows = Vec::new();

                for right_row in &right_chunk.rows {
                    let mut matched = false;
                    for left_row in build_side_tuples.iter() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let mut combined_col_names = right_col_names.clone();
                            for i in 0..left_row.len() {
                                combined_col_names.push(format!("left_{}", i));
                            }
                            let mut context = ValueRowContext::new(combined_row, combined_col_names);
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
                        for _ in 0..build_side_tuples.get(0).map(|r| r.len()).unwrap_or(0) {
                            unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                        }
                        unmatched_row.extend(right_row.clone());
                        result_rows.push(unmatched_row);
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_rightjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::RightJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ FullOuterJoin ============

pub fn open_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            opened,
            phase,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::BuildingRight;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_fullouterjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            join_condition,
            left_rows,
            right_rows,
            matched_right_indices,
            result_iter,
            phase,
            ..
        } => {
            loop {
                match phase {
                    crate::query::executor::streaming::executor::FullOuterJoinPhase::BuildingRight => {
                        // Collect all left and right rows
                        while let Some(chunk) = left.next()? {
                            for row in chunk.rows {
                                left_rows.push(row);
                            }
                        }
                        while let Some(chunk) = right.next()? {
                            for row in chunk.rows {
                                right_rows.push(row);
                            }
                        }
                        *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::ProbeLeft;
                    }

                    crate::query::executor::streaming::executor::FullOuterJoinPhase::ProbeLeft => {
                        let right_col_count = right_rows.get(0).map(|r| r.len()).unwrap_or(0);
                        let mut all_results = Vec::new();

                        for left_row in left_rows.iter() {
                            let mut matched = false;
                            for (right_idx, right_row) in right_rows.iter().enumerate() {
                                let condition_satisfied = if let Some(condition) = join_condition {
                                    let left_col_names: Vec<String> = (0..left_row.len()).map(|i| format!("col_{}", i)).collect();
                                    let mut combined_row = left_row.clone();
                                    combined_row.extend(right_row.clone());
                                    let mut combined_col_names = left_col_names.clone();
                                    for i in 0..right_row.len() {
                                        combined_col_names.push(format!("right_{}", i));
                                    }
                                    let mut context = ValueRowContext::new(combined_row, combined_col_names);
                                    match ExpressionEvaluator::evaluate(condition, &mut context) {
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
                                    unmatched_row.push(Value::Null(crate::core::value::NullType::Null));
                                }
                                all_results.push(unmatched_row);
                            }
                        }

                        *phase = crate::query::executor::streaming::executor::FullOuterJoinPhase::EmitUnmatchedRight;
                        if !all_results.is_empty() {
                            *result_iter = Some(all_results.into_iter());
                        }
                        continue;
                    }

                    crate::query::executor::streaming::executor::FullOuterJoinPhase::EmitUnmatchedRight => {
                        // Drain buffered matched+unmatched-left results first
                        if let Some(iter) = result_iter {
                            let rows: Vec<Vec<Value>> = iter.collect();
                            if !rows.is_empty() {
                                return Ok(Some(DataChunk::from_rows(rows)));
                            }
                            *result_iter = None;
                        }

                        // Emit unmatched right rows
                        let left_col_count = left_rows.get(0).map(|r| r.len()).unwrap_or(0);
                        let mut unmatched = Vec::new();
                        for (right_idx, right_row) in right_rows.iter().enumerate() {
                            if !matched_right_indices.contains(&right_idx) {
                                let mut row = Vec::new();
                                for _ in 0..left_col_count {
                                    row.push(Value::Null(crate::core::value::NullType::Null));
                                }
                                row.extend(right_row.clone());
                                unmatched.push(row);
                            }
                        }

                        if unmatched.is_empty() {
                            return Ok(None);
                        }
                        return Ok(Some(DataChunk::from_rows(unmatched)));
                    }
                }
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_fullouterjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::FullOuterJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ CrossJoin ============

pub fn open_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_crossjoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            all_left_rows,
            all_right_rows,
            left_consumed,
            right_consumed,
            ..
        } => {
            if !*left_consumed {
                while let Some(chunk) = left.next()? {
                    for row in chunk.rows {
                        all_left_rows.push(row);
                    }
                }
                *left_consumed = true;
            }

            if !*right_consumed {
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        all_right_rows.push(row);
                    }
                }
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
                Ok(Some(DataChunk::from_rows(result_rows)))
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_crossjoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::CrossJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

// ============ SemiJoin ============

pub fn open_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            opened,
            ..
        } => {
            left.open()?;
            right.open()?;
            *opened = true;
            Ok(())
        }
        _ => unreachable!(),
    }
}

pub fn next_semijoin(executor: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            join_condition,
            right_rows,
            right_consumed,
            ..
        } => {
            if !*right_consumed {
                while let Some(chunk) = right.next()? {
                    for row in chunk.rows {
                        right_rows.push(row);
                    }
                }
                *right_consumed = true;
            }

            if let Some(left_chunk) = left.next()? {
                let left_col_names = left_chunk.col_names();
                let mut result_rows = Vec::new();

                for left_row in &left_chunk.rows {
                    for right_row in right_rows.iter() {
                        let condition_satisfied = if let Some(condition) = join_condition {
                            let mut combined_row = left_row.clone();
                            combined_row.extend(right_row.clone());
                            let mut combined_col_names = left_col_names.clone();
                            for i in 0..right_row.len() {
                                combined_col_names.push(format!("right_{}", i));
                            }
                            let mut context = ValueRowContext::new(combined_row, combined_col_names);
                            match ExpressionEvaluator::evaluate(condition, &mut context) {
                                Ok(Value::Bool(b)) => b,
                                _ => false,
                            }
                        } else {
                            true
                        };

                        if condition_satisfied {
                            result_rows.push(left_row.clone());
                            break; // SemiJoin only returns one copy of left row
                        }
                    }
                }

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(DataChunk::from_rows(result_rows)))
                }
            } else {
                Ok(None)
            }
        }
        _ => unreachable!(),
    }
}

pub fn stop_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin { left, right, .. } => {
            left.stop()?;
            right.stop()
        }
        _ => unreachable!(),
    }
}

pub fn close_semijoin(executor: &mut StreamingExecutor) -> Result<(), QueryError> {
    match executor {
        StreamingExecutor::SemiJoin {
            left,
            right,
            opened,
            ..
        } => {
            if *opened {
                left.close()?;
                right.close()?;
                *opened = false;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::expr::Expression;
    use crate::core::value::NullType;

    fn create_left_buffer() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Int(1), Value::String("a".to_string())],
            vec![Value::Int(2), Value::String("b".to_string())],
        ]
    }

    fn create_right_buffer() -> Vec<Vec<Value>> {
        vec![
            vec![Value::Int(1), Value::String("x".to_string())],
            vec![Value::Int(2), Value::String("y".to_string())],
            vec![Value::Int(3), Value::String("z".to_string())],
        ]
    }

    #[test]
    fn test_hashjoin_basic() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: create_left_buffer(),
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: create_right_buffer(),
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 3 right rows = 6 result rows
        assert_eq!(chunk.unwrap().len(), 6);
        join.close().unwrap();
    }

    #[test]
    fn test_hashjoin_no_match() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(10), Value::String("a".to_string())]],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(20), Value::String("b".to_string())]],
            current_index: 0,
        });

        let join_condition = Some(Expression::Literal(Value::Bool(false)));

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition,
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_none());
        join.close().unwrap();
    }

    #[test]
    fn test_hashjoin_multi_match() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("a1".to_string())],
                vec![Value::Int(1), Value::String("a2".to_string())],
            ],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::String("b1".to_string())],
                vec![Value::Int(1), Value::String("b2".to_string())],
            ],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 2 right rows = 4 result rows
        assert_eq!(chunk.unwrap().len(), 4);
        join.close().unwrap();
    }

    #[test]
    fn test_nestedloop_cartesian() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
            ],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(10)],
                vec![Value::Int(20)],
                vec![Value::Int(30)],
            ],
            current_index: 0,
        });

        let mut join = StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition: None,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 × 3 = 6 rows
        assert_eq!(chunk.unwrap().len(), 6);
        join.close().unwrap();
    }

    #[test]
    fn test_nestedloop_condition() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            current_index: 0,
        });

        // Condition: always true
        let join_condition = Some(Expression::Literal(Value::Bool(true)));

        let mut join = StreamingExecutor::NestedLoopJoin {
            left,
            right,
            join_condition,
            build_side_tuples: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // 2 × 2 = 4 rows
        assert_eq!(chunk.unwrap().len(), 4);
        join.close().unwrap();
    }

    #[test]
    fn test_join_null() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![
                vec![Value::Int(1), Value::Null(NullType::Null)],
                vec![Value::Int(2), Value::String("b".to_string())],
            ],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::String("x".to_string()), Value::Int(10)]],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        // Cartesian product: 2 left rows × 1 right row = 2 result rows
        assert_eq!(chunk.unwrap().len(), 2);
        join.close().unwrap();
    }

    #[test]
    fn test_join_column_naming() {
        let left = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(1), Value::String("left".to_string())]],
            current_index: 0,
        });

        let right = Box::new(StreamingExecutor::ScanVertices {
            partition_id: 0,
            buffer: vec![vec![Value::Int(2), Value::String("right".to_string())]],
            current_index: 0,
        });

        let mut join = StreamingExecutor::HashJoin {
            left,
            right,
            join_condition: None,
            build_side_hash: std::collections::HashMap::new(),
            all_right_rows: Vec::new(),
            left_consumed: false,
            opened: false,
        };

        join.open().unwrap();
        let chunk = join.next().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        // Result row should have 4 columns (2 from left + 2 from right)
        assert_eq!(chunk.rows[0].len(), 4);
        join.close().unwrap();
    }
}


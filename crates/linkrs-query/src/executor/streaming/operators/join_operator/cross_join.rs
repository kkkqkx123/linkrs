use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use super::finalize_join_output;

/// Rows emitted per cross-join output chunk.
///
/// The cartesian product is streamed in bounded chunks instead of being
/// materialized as one huge chunk, so a wide product costs a bounded amount
/// of memory.
const CROSS_JOIN_CHUNK_ROWS: usize = 1024;

#[allow(clippy::too_many_arguments)]
pub(super) fn next_cross_join(
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
    left_consumed: &mut bool,
    right_consumed: &mut bool,
    cursor: &mut (usize, usize),
    memory_tracker: &mut MemoryTracker,
    right_col_names: &mut Vec<String>,
    left: &mut StreamingExecutor,
    right: &mut StreamingExecutor,
    runtime: &Option<Arc<ExecutionRuntime>>,
    output_layout: &Arc<SlotLayout>,
) -> Result<Option<DataChunk>, QueryError> {
    // Both inputs are buffered once; the product is then emitted chunk by
    // chunk. Subsequent pulls continue at the cursor instead of re-emitting
    // the product.
    if !*left_consumed {
        while let Some(mut chunk) = left.advance()? {
            chunk.normalize_for_opaque("CrossJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            for row in &chunk.rows {
                memory_tracker.try_reserve_row(row)?;
            }
            all_left_rows.extend(chunk.rows);
        }
        *left_consumed = true;
    }

    if !*right_consumed {
        let mut captured_right_names = Vec::new();
        while let Some(mut chunk) = right.advance()? {
            chunk.normalize_for_opaque("CrossJoin");
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
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
        *cursor = (0, 0);
        return Ok(None);
    }

    let (mut left_index, mut right_index) = *cursor;
    let mut result_rows = Vec::with_capacity(CROSS_JOIN_CHUNK_ROWS);
    while left_index < all_left_rows.len() && result_rows.len() < CROSS_JOIN_CHUNK_ROWS {
        let left_row = &all_left_rows[left_index];
        while right_index < all_right_rows.len() && result_rows.len() < CROSS_JOIN_CHUNK_ROWS {
            if let Some(rt) = runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            let mut joined_row = left_row.clone();
            joined_row.extend(all_right_rows[right_index].clone());
            result_rows.push(joined_row);
            right_index += 1;
        }
        if right_index >= all_right_rows.len() {
            left_index += 1;
            right_index = 0;
        }
    }
    *cursor = (left_index, right_index);

    if result_rows.is_empty() {
        return Ok(None);
    }
    Ok(Some(finalize_join_output(
        DataChunk::new_with_layout(result_rows, Arc::clone(output_layout)),
        runtime,
    )))
}

pub(super) fn close_cross(
    memory_tracker: &mut MemoryTracker,
    all_left_rows: &mut Vec<Vec<Value>>,
    all_right_rows: &mut Vec<Vec<Value>>,
) -> Result<(), QueryError> {
    memory_tracker.reset();
    all_left_rows.clear();
    all_right_rows.clear();
    Ok(())
}

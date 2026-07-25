use std::hash::Hasher;

use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::context::BorrowedRowContext;
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::helpers::compare_values;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::spill::{RunReader, SpillManager, SpilledFile, SpilledRun};

#[derive(Debug)]
pub struct SortState {
    pub col_names: Vec<String>,
    pub input_layout: Option<Arc<SlotLayout>>,
    pub all_rows: Vec<Vec<Value>>,
    pub row_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
    pub runs: Vec<SpilledRun>,
    pub has_spilled: bool,
    pub merge_state: Option<MergeState>,
}

#[derive(Debug)]
pub struct MergeState {
    pub run_buffers: Vec<RunBuffer>,
    pub col_names: Vec<String>,
}

#[derive(Debug)]
pub struct RunBuffer {
    pub rows: Vec<Vec<Value>>,
    pub index: usize,
    pub reader: RunReader,
}

#[derive(Debug)]
pub struct TopNState {
    pub all_rows: Vec<Vec<Value>>,
    pub col_names: Vec<String>,
    pub input_layout: Option<Arc<SlotLayout>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
}

pub(crate) fn spill_sorted_run(
    buffer: &mut Vec<Vec<Value>>,
    col_names: &[String],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
    sm: &SpillManager,
    tracker: &mut MemoryTracker,
    runs: &mut Vec<SpilledRun>,
) -> Result<u64, QueryError> {
    if buffer.is_empty() {
        return Ok(0);
    }

    sort_rows(buffer, col_names, sort_expressions, sort_directions);

    let fp = compute_schema_fingerprint(col_names);

    let estimated_bytes = estimate_run_size(buffer, col_names);
    sm.disk_quota().try_reserve(estimated_bytes)?;

    let mut writer = sm.create_run_writer(fp)?;
    writer.write_rows(buffer)?;
    let run = writer.finalize()?;

    let count = buffer.len() as u64;
    buffer.clear();
    tracker.reset();
    runs.push(run);
    Ok(count)
}

pub(crate) fn sort_rows(
    buffer: &mut [Vec<Value>],
    col_names: &[String],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) {
    if sort_expressions.is_empty() || buffer.len() <= 1 {
        return;
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));

    let mut ctx = BorrowedRowContext::new(&buffer[0], Arc::clone(&layout));
    let sort_keys: Vec<Vec<Value>> = buffer
        .iter()
        .enumerate()
        .map(|(i, row)| {
            if i > 0 {
                ctx.set_row(row);
            }
            sort_expressions
                .iter()
                .map(|expr| {
                    ExpressionEvaluator::evaluate(expr, &mut ctx)
                        .unwrap_or(Value::Null(NullType::Null))
                })
                .collect()
        })
        .collect();

    let mut indices: Vec<usize> = (0..buffer.len()).collect();
    indices.sort_by(|&i, &j| {
        for (idx, _) in sort_expressions.iter().enumerate() {
            let direction = sort_directions
                .get(idx)
                .copied()
                .unwrap_or(SortDirection::Ascending);

            let cmp = compare_values(&sort_keys[i][idx], &sort_keys[j][idx]);
            let final_cmp = match direction {
                SortDirection::Ascending => cmp,
                SortDirection::Descending => cmp.reverse(),
            };

            if final_cmp != std::cmp::Ordering::Equal {
                return final_cmp;
            }
        }
        std::cmp::Ordering::Equal
    });

    let mut sorted = Vec::with_capacity(buffer.len());
    for &i in &indices {
        sorted.push(std::mem::take(&mut buffer[i]));
    }
    buffer.clone_from_slice(&sorted);
}

fn compute_schema_fingerprint(col_names: &[String]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for name in col_names {
        hasher.write(name.as_bytes());
        hasher.write_u8(0);
    }
    hasher.finish()
}

fn estimate_run_size(buffer: &[Vec<Value>], _col_names: &[String]) -> u64 {
    let per_row_overhead: u64 = 8;
    let data_bytes: u64 = buffer
        .iter()
        .map(|r| r.iter().map(|v| v.estimated_size() as u64 + 1).sum::<u64>())
        .sum();
    40 + data_bytes + per_row_overhead * buffer.len() as u64
}

pub(crate) fn find_min_run(
    run_buffers: &[RunBuffer],
    col_names: &[String],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) -> Option<usize> {
    let mut best_idx: Option<usize> = None;
    let mut best_row: Option<&[Value]> = None;

    if sort_expressions.is_empty() {
        return None;
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));

    for (i, buf) in run_buffers.iter().enumerate() {
        if buf.index < buf.rows.len() {
            let row = &buf.rows[buf.index];
            if best_idx.is_none() {
                best_idx = Some(i);
                best_row = Some(row);
            } else if let Some(best) = best_row {
                let cmp = compare_two_rows_for_merge(
                    row,
                    best,
                    &layout,
                    sort_expressions,
                    sort_directions,
                );
                if cmp == std::cmp::Ordering::Less {
                    best_idx = Some(i);
                    best_row = Some(row);
                }
            }
        }
    }
    best_idx
}

pub(crate) fn compare_two_rows_for_merge(
    a: &[Value],
    b: &[Value],
    layout: &Arc<SlotLayout>,
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) -> std::cmp::Ordering {
    for (idx, expr) in sort_expressions.iter().enumerate() {
        let direction = sort_directions
            .get(idx)
            .copied()
            .unwrap_or(SortDirection::Ascending);

        let mut ctx_a = BorrowedRowContext::new(a, Arc::clone(layout));
        let mut ctx_b = BorrowedRowContext::new(b, Arc::clone(layout));

        let val_a =
            ExpressionEvaluator::evaluate(expr, &mut ctx_a).unwrap_or(Value::Null(NullType::Null));
        let val_b =
            ExpressionEvaluator::evaluate(expr, &mut ctx_b).unwrap_or(Value::Null(NullType::Null));

        let cmp = compare_values(&val_a, &val_b);

        let final_cmp = match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        };

        if final_cmp != std::cmp::Ordering::Equal {
            return final_cmp;
        }
    }
    std::cmp::Ordering::Equal
}

pub(crate) fn refill_run_buffer(buf: &mut RunBuffer, batch_size: usize) -> Result<(), QueryError> {
    buf.rows.clear();
    buf.index = 0;
    for _ in 0..batch_size {
        match buf.reader.read_row()? {
            Some(row) => buf.rows.push(row),
            None => break,
        }
    }
    Ok(())
}

pub(crate) fn compare_rows_for_topn(
    a: &[Value],
    b: &[Value],
    layout: &Arc<SlotLayout>,
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) -> std::cmp::Ordering {
    for (idx, expr) in sort_expressions.iter().enumerate() {
        let direction = sort_directions
            .get(idx)
            .copied()
            .unwrap_or(SortDirection::Ascending);

        let mut ctx_a = BorrowedRowContext::new(a, Arc::clone(layout));
        let mut ctx_b = BorrowedRowContext::new(b, Arc::clone(layout));

        let val_a =
            ExpressionEvaluator::evaluate(expr, &mut ctx_a).unwrap_or(Value::Null(NullType::Null));
        let val_b =
            ExpressionEvaluator::evaluate(expr, &mut ctx_b).unwrap_or(Value::Null(NullType::Null));

        let cmp = compare_values(&val_a, &val_b);

        let final_cmp = match direction {
            SortDirection::Ascending => cmp,
            SortDirection::Descending => cmp.reverse(),
        };

        if final_cmp != std::cmp::Ordering::Equal {
            return final_cmp;
        }
    }
    std::cmp::Ordering::Equal
}

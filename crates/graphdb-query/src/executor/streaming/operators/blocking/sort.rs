use std::hash::Hasher;

use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::ColumnarBatch;
use crate::executor::streaming::context::BorrowedRowContext;
use crate::executor::streaming::executor::SortDirection;
use crate::executor::streaming::helpers::compare_values;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::spill::{RunReader, SpillManager, SpilledFile, SpilledRun};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::value::NullType;
use graphdb_core::Value;

#[derive(Debug)]
pub struct SortState {
    pub col_names: Vec<String>,
    pub input_layout: Option<Arc<SlotLayout>>,
    /// Columnar accumulation of the in-memory prefix (below the spill
    /// boundary). Once spilled, remaining rows are materialized and handed
    /// to the row-based `spill_sorted_run`/merge machinery.
    pub columnar_batch: Option<ColumnarBatch>,
    /// Row-mode fallback buffer (used once a spill occurred).
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
    /// Bounded columnar accumulation (kept at most `n` rows after each
    /// chunk append).
    pub columnar_batch: Option<ColumnarBatch>,
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

/// Resolve a sort expression that references a bare column (a `Variable` or
/// a flat `Property` access) to its column index in `col_names`.
///
/// Bare column references compare directly on the typed raw values of a
/// [`ColumnarBatch`]; anything else needs per-row expression evaluation.
fn bare_column_index(expr: &Expression, col_names: &[String]) -> Option<usize> {
    match expr {
        Expression::Variable(name) => col_names.iter().position(|c| c == name),
        Expression::Property { object, property } => {
            if let Expression::Variable(var) = object.as_ref() {
                let compound = format!("{}.{}", var, property);
                col_names.iter().position(|c| c == &compound)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Precompute per-row sort keys for expressions that are not bare column
/// references (evaluated once per row, matching `sort_rows`).
fn eval_row_sort_keys(
    rows: &[Vec<Value>],
    col_names: &[String],
    sort_expressions: &[Expression],
) -> Vec<Vec<Value>> {
    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut ctx = BorrowedRowContext::new(
        rows.first().map_or(&[], |r| r.as_slice()),
        Arc::clone(&layout),
    );
    rows.iter()
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
        .collect()
}

/// Columnar in-place sort of a [`ColumnarBatch`].
///
/// Sort keys that are bare column references compare directly on the typed
/// raw column values (no per-row `Value` construction); remaining
/// expressions are evaluated once per row (same semantics as `sort_rows`).
/// The batch is reordered in place via a permutation.
///
/// Single-column integer sorts use a radix fast path (LSD
/// 8-pass counting sort, O(n)) instead of comparison sort. The radix path
/// is selected only for `I64`/`I32` columns in ascending order with no NULL
/// bitmap (NULLs sort last via the comparison path).
pub(crate) fn sort_columnar_batch(
    batch: &mut ColumnarBatch,
    col_names: &[String],
    sort_expressions: &[Expression],
    sort_directions: &[SortDirection],
) {
    if sort_expressions.is_empty() || batch.num_rows() <= 1 {
        return;
    }

    let key_cols: Vec<Option<usize>> = sort_expressions
        .iter()
        .map(|expr| bare_column_index(expr, col_names))
        .collect();
    // Radix fast path: single bare `I64`/`I32` column, ascending,
    // non-nullable, large batch.
    if key_cols.len() == 1
        && key_cols[0].is_some()
        && sort_directions
            .first()
            .copied()
            .unwrap_or(SortDirection::Ascending)
            == SortDirection::Ascending
    {
        let col = key_cols[0].unwrap();
        if batch.num_rows() > 2048 && try_radix_sort(batch, col) {
            return;
        }
    }
    let needs_row_keys = key_cols.iter().any(Option::is_none);

    // Expressions that are not bare column references are evaluated once per
    // row and compared as `Value`s.
    let row_keys: Vec<Vec<Value>> = if needs_row_keys {
        let rows = batch.to_rows();
        eval_row_sort_keys(&rows, col_names, sort_expressions)
    } else {
        Vec::new()
    };

    let mut indices: Vec<usize> = (0..batch.num_rows()).collect();
    indices.sort_by(|&i, &j| {
        for (k, _) in sort_expressions.iter().enumerate() {
            let direction = sort_directions
                .get(k)
                .copied()
                .unwrap_or(SortDirection::Ascending);

            let cmp = match key_cols[k] {
                Some(col) => batch.compare_rows_at(col, i, j),
                None => compare_values(&row_keys[i][k], &row_keys[j][k]),
            };
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

    batch.permute(&indices);
}

/// Attempt LSD radix sort for a single `I64`/`I32` column (ascending,
/// non-nullable). Returns `true` when the fast path was taken.
fn try_radix_sort(batch: &mut ColumnarBatch, col_idx: usize) -> bool {
    let col = batch.column(col_idx);
    match col {
        crate::executor::streaming::chunk::BatchColumn::I64(vals) => {
            let perm = radix_sort_i64(vals);
            batch.permute(&perm);
            true
        }
        crate::executor::streaming::chunk::BatchColumn::I32(vals) => {
            let vals_i64: Vec<i64> = vals.iter().map(|&x| x as i64).collect();
            let perm = radix_sort_i64(&vals_i64);
            batch.permute(&perm);
            true
        }
        _ => false,
    }
}

/// LSD radix sort (8 passes of 8 bits) for `Vec<i64>` keys.
///
/// Stability is preserved (counting sort is stable). Negative values are
/// handled by flipping the sign bit so the unsigned order matches signed
/// order. Returns the permutation that sorts `keys`.
fn radix_sort_i64(keys: &[i64]) -> Vec<usize> {
    let n = keys.len();
    if n <= 1 {
        return (0..n).collect();
    }
    let mut indices: Vec<usize> = (0..n).collect();
    let mut aux = vec![0usize; n];
    // Transform to unsigned with sign-bit flipped for correct signed ordering.
    let mut cur_keys: Vec<u64> = keys.iter().map(|&x| (x as u64) ^ (1u64 << 63)).collect();
    let mut cur_aux = vec![0u64; n];
    for shift in (0..64).step_by(8) {
        let mut count = [0usize; 256];
        for &k in &cur_keys {
            count[((k >> shift) & 0xFF) as usize] += 1;
        }
        let mut pos = [0usize; 256];
        let mut sum = 0usize;
        for i in 0..256 {
            pos[i] = sum;
            sum += count[i];
        }
        for &idx in &indices {
            let bucket = ((cur_keys[idx] >> shift) & 0xFF) as usize;
            aux[pos[bucket]] = idx;
            cur_aux[pos[bucket]] = cur_keys[idx];
            pos[bucket] += 1;
        }
        std::mem::swap(&mut indices, &mut aux);
        std::mem::swap(&mut cur_keys, &mut cur_aux);
    }
    // After 8 even passes (64/8=8) the indices are back in `indices`.
    indices
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
    let mut ctx_a = BorrowedRowContext::new(a, Arc::clone(layout));
    let mut ctx_b = BorrowedRowContext::new(b, Arc::clone(layout));

    for (idx, expr) in sort_expressions.iter().enumerate() {
        let direction = sort_directions
            .get(idx)
            .copied()
            .unwrap_or(SortDirection::Ascending);

        if idx > 0 {
            ctx_a.set_row(a);
            ctx_b.set_row(b);
        }

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

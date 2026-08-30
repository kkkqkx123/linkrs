use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::Value;

use crate::executor::streaming::chunk::ColumnarBatch;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::blocking::sort::{
    find_min_run, refill_run_buffer, sort_columnar_batch, spill_sorted_run, MergeState, RunBuffer,
    SortState, TopNState,
};
use crate::executor::streaming::spill::RunReader;

use super::helpers::BlockingContext;

pub(super) fn open_sort(state: &mut Option<SortState>) {
    *state = Some(SortState {
        col_names: vec![],
        input_layout: None,
        columnar_batch: None,
        all_rows: vec![],
        row_iter: None,
        spill_files: vec![],
        runs: vec![],
        has_spilled: false,
        merge_state: None,
    });
}

pub(super) fn open_topn(state: &mut Option<TopNState>) {
    *state = Some(TopNState {
        columnar_batch: None,
        col_names: vec![],
        input_layout: None,
        result_iter: None,
    });
}

pub(super) fn next_sort(
    sort_expressions: &[graphdb_core::types::expr::Expression],
    sort_directions: &[crate::executor::streaming::executor::SortDirection],
    memory_tracker: &mut crate::executor::base::MemoryTracker,
    state: &mut SortState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<crate::executor::streaming::chunk::DataChunk>, QueryError> {
    if state.merge_state.is_none() && state.row_iter.is_none() {
        while let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("Sort");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if state.col_names.is_empty() {
                state.col_names = chunk.col_names();
                state.input_layout = Some(chunk.get_layout());
            }
            let batch = state
                .columnar_batch
                .get_or_insert_with(|| ColumnarBatch::new(chunk.num_columns()));
            for idx in chunk.visible_indices() {
                let row = &chunk.rows[idx];
                if let Err(e) = memory_tracker.try_reserve_row(row) {
                    if let Some(sm) = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
                        let mut rows = batch.to_rows();
                        spill_sorted_run(
                            &mut rows,
                            &state.col_names,
                            sort_expressions,
                            sort_directions,
                            &sm,
                            memory_tracker,
                            &mut state.runs,
                        )?;
                        state.has_spilled = true;
                        batch.clear();
                        memory_tracker.reset();
                        memory_tracker.try_reserve_row(row)?;
                    } else {
                        return Err(e);
                    }
                }
                batch.append_chunk_row(&chunk, idx);
            }
        }

        if !state.spill_files.is_empty() {
            return super::helpers::reject_spill_replay(&state.spill_files).map(|_| None);
        }

        if state.has_spilled {
            if let Some(batch) = state.columnar_batch.take() {
                if batch.num_rows() > 0 {
                    if let Some(sm) = ctx.runtime.as_ref().and_then(|rt| rt.get_spill_manager()) {
                        let mut rows = batch.to_rows();
                        spill_sorted_run(
                            &mut rows,
                            &state.col_names,
                            sort_expressions,
                            sort_directions,
                            &sm,
                            memory_tracker,
                            &mut state.runs,
                        )?;
                    }
                }
            }

            let mut run_buffers = Vec::with_capacity(state.runs.len());
            for run in &state.runs {
                let reader = RunReader::open(run)?;
                run_buffers.push(RunBuffer {
                    rows: Vec::new(),
                    index: 0,
                    reader,
                });
            }

            for buf in &mut run_buffers {
                refill_run_buffer(buf, 2048)?;
            }

            state.merge_state = Some(MergeState {
                run_buffers,
                col_names: state.col_names.clone(),
            });
        } else {
            if let Some(mut batch) = state.columnar_batch.take() {
                if !sort_expressions.is_empty() {
                    sort_columnar_batch(
                        &mut batch,
                        &state.col_names,
                        sort_expressions,
                        sort_directions,
                    );
                }
                state.row_iter = Some(batch.to_rows().into_iter());
            }
        }
    }

    if let Some(ref mut merge) = state.merge_state {
        let batch_size = 1024;
        let mut out_rows = Vec::with_capacity(batch_size);

        while out_rows.len() < batch_size {
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            let min_idx = find_min_run(
                &merge.run_buffers,
                &merge.col_names,
                sort_expressions,
                sort_directions,
            );

            match min_idx {
                None => break,
                Some(idx) => {
                    let buf = &mut merge.run_buffers[idx];
                    let row = buf.rows[buf.index].clone();
                    out_rows.push(row);
                    buf.index += 1;

                    if buf.index >= buf.rows.len() {
                        refill_run_buffer(buf, 2048)?;
                    }
                }
            }
        }

        if out_rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                crate::executor::streaming::chunk::DataChunk::new_with_layout(
                    out_rows,
                    Arc::clone(ctx.output_layout),
                ),
            ))
        }
    } else if let Some(ref mut iter) = state.row_iter {
        let chunk_rows: Vec<Vec<Value>> = iter.by_ref().take(2048).collect();
        if chunk_rows.is_empty() {
            Ok(None)
        } else {
            Ok(Some(
                crate::executor::streaming::chunk::DataChunk::new_with_layout(
                    chunk_rows,
                    Arc::clone(ctx.output_layout),
                ),
            ))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn next_topn(
    n: u32,
    sort_expressions: &[graphdb_core::types::expr::Expression],
    sort_directions: &[crate::executor::streaming::executor::SortDirection],
    memory_tracker: &mut crate::executor::base::MemoryTracker,
    state: &mut TopNState,
    ctx: &BlockingContext<'_>,
    input: &mut StreamingExecutor,
) -> Result<Option<crate::executor::streaming::chunk::DataChunk>, QueryError> {
    if state.result_iter.is_none() {
        let limit = n as usize;

        while let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("TopN");
            if let Some(rt) = ctx.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            if state.col_names.is_empty() {
                state.col_names = chunk.col_names();
                state.input_layout = Some(chunk.get_layout());
            }
            let batch = state
                .columnar_batch
                .get_or_insert_with(|| ColumnarBatch::new(chunk.num_columns()));
            for idx in chunk.visible_indices() {
                memory_tracker.try_reserve_row(&chunk.rows[idx])?;
                batch.append_chunk_row(&chunk, idx);
            }
            if batch.num_rows() > limit {
                if !sort_expressions.is_empty() {
                    sort_columnar_batch(batch, &state.col_names, sort_expressions, sort_directions);
                }
                batch.truncate(limit);
            }
        }

        if let Some(mut batch) = state.columnar_batch.take() {
            if !sort_expressions.is_empty() && batch.num_rows() > 1 {
                sort_columnar_batch(
                    &mut batch,
                    &state.col_names,
                    sort_expressions,
                    sort_directions,
                );
            }
            state.result_iter = Some(batch.to_rows().into_iter());
        } else {
            state.result_iter = Some(Vec::new().into_iter());
        }
    }

    if let Some(iter) = &mut state.result_iter {
        if let Some(row) = iter.next() {
            Ok(Some(
                crate::executor::streaming::chunk::DataChunk::new_with_layout(
                    vec![row],
                    Arc::clone(ctx.output_layout),
                ),
            ))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

pub(super) fn close_sort(state: &mut Option<SortState>) {
    if let Some(ref s) = state {
        for run in &s.runs {
            let _ = std::fs::remove_file(&run.path);
        }
        for sf in &s.spill_files {
            let _ = std::fs::remove_file(&sf.path);
        }
    }
    *state = None;
}

pub(super) fn close_topn(state: &mut Option<TopNState>) {
    *state = None;
}

pub(super) fn spill_sort(
    state: &mut SortState,
    memory_tracker: &mut crate::executor::base::MemoryTracker,
    sort_expressions: &[graphdb_core::types::expr::Expression],
    sort_directions: &[crate::executor::streaming::executor::SortDirection],
    sm: &crate::executor::streaming::spill::SpillManager,
) -> Result<(), QueryError> {
    if !state.all_rows.is_empty() {
        spill_sorted_run(
            &mut state.all_rows,
            &state.col_names,
            sort_expressions,
            sort_directions,
            sm,
            memory_tracker,
            &mut state.runs,
        )?;
        state.has_spilled = true;
    }
    Ok(())
}

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::chunk::{gather_typed_column, VECTORIZED_BATCH_SIZE};
use crate::executor::streaming::executor::StreamingExecutor;
use graphdb_core::error::QueryError;

/// Default batch size for the batched flatten path.
///
/// Kept in sync with the columnar `VECTORIZED_BATCH_SIZE` so one flatten
/// output never exceeds a single vectorized morsel.
pub const DEFAULT_FLATTEN_BATCH_SIZE: usize = VECTORIZED_BATCH_SIZE;

/// Shared flatten logic used by `UnaryOperatorKind::Flatten`.
/// Production streaming uses `UnaryOperatorKind::Flatten` via `unary_operator.rs`.
/// This module provides the single-row `flatten_next_inner` implementation
/// on top of the batched `flatten_next_batch` core.
///
/// The streaming engine stores flat row batches only. There is no nested
/// vector layout behind `group_pos`, so flatten replays visible rows in order
/// instead of expanding one factorization group.
pub(crate) fn prepare_flatten_buffer(chunk: DataChunk) -> (Vec<usize>, DataChunk) {
    let sel = chunk.visible_indices();
    let mut buffered = chunk;
    let _ = buffered.take_selection();
    (sel, buffered)
}

pub(crate) fn flatten_next_inner(
    current_idx: &mut usize,
    size_to_flatten: &mut usize,
    saved_sel_vector: &mut Option<Vec<usize>>,
    buffered_chunk: &mut Option<DataChunk>,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    flatten_next_batch(
        current_idx,
        size_to_flatten,
        saved_sel_vector,
        buffered_chunk,
        input,
        1,
    )
}

/// Batched flatten: emit up to `batch_size` visible rows per output chunk.
///
/// The caller-visible state machine (`current_idx` / `size_to_flatten` /
/// `saved_sel_vector` / `buffered_chunk`) is shared with the single-row
/// path, so both paths observe identical row order and identical
/// `typed_columns` content: each output column is gathered with the
/// columnar `gather_typed_column` helper over the emitted selection
/// slice instead of per-row clones.
pub(crate) fn flatten_next_batch(
    current_idx: &mut usize,
    size_to_flatten: &mut usize,
    saved_sel_vector: &mut Option<Vec<usize>>,
    buffered_chunk: &mut Option<DataChunk>,
    input: &mut StreamingExecutor,
    batch_size: usize,
) -> Result<Option<DataChunk>, QueryError> {
    let batch_size = batch_size.max(1);
    loop {
        if buffered_chunk.is_some() {
            if *current_idx < *size_to_flatten {
                let take = (*size_to_flatten - *current_idx).min(batch_size);
                // Borrow scope ends before the state mutation below, so the
                // retained buffered chunk is never cloned back into place.
                let out = {
                    let chunk = buffered_chunk
                        .as_ref()
                        .expect("buffered chunk must be present");
                    let sel_vec = saved_sel_vector
                        .as_ref()
                        .expect("saved sel vector must be present");
                    let positions: Vec<usize> = sel_vec[*current_idx..*current_idx + take].to_vec();
                    build_flatten_batch_chunk(chunk, &positions)
                };
                *current_idx += take;
                if *current_idx >= *size_to_flatten {
                    *buffered_chunk = None;
                    *saved_sel_vector = None;
                    *size_to_flatten = 0;
                    *current_idx = 0;
                }
                return Ok(Some(out));
            } else {
                *saved_sel_vector = None;
                *size_to_flatten = 0;
                *current_idx = 0;
            }
        }
        let child_chunk = match input.advance()? {
            Some(c) => c,
            None => return Ok(None),
        };
        if child_chunk.visible_count() == 0 {
            continue;
        }
        let (sel, buffered) = prepare_flatten_buffer(child_chunk);
        *size_to_flatten = sel.len();
        *saved_sel_vector = Some(sel);
        *current_idx = 0;
        *buffered_chunk = Some(buffered);
    }
}

/// Materialize one flatten output chunk for `positions` of `chunk`.
///
/// Row order follows `positions`; `columns` and `typed_columns` are
/// gathered column-wise so the vectorized layout is preserved end to end.
fn build_flatten_batch_chunk(chunk: &DataChunk, positions: &[usize]) -> DataChunk {
    let layout = chunk.get_layout();
    let schema = chunk.schema.clone();
    let rows = positions
        .iter()
        .map(|&sel_pos| chunk.rows[sel_pos].clone())
        .collect::<Vec<_>>();
    let columns = chunk.columns.as_ref().map(|cols| {
        cols.iter()
            .map(|col| {
                positions
                    .iter()
                    .map(|&sel_pos| col[sel_pos].clone())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    });
    let typed_columns = chunk.typed_columns.as_ref().map(|cols| {
        cols.iter()
            .map(|col| gather_typed_column(col, positions))
            .collect::<Vec<_>>()
    });
    let columnar_stats = chunk.columnar_stats.clone();
    let mut out = DataChunk::new_with_layout(rows, layout);
    out.schema = schema;
    out.columns = columns;
    out.typed_columns = typed_columns;
    out.columnar_stats = columnar_stats;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::chunk::DataChunk;
    use crate::executor::streaming::executor::StreamingExecutor;
    use crate::executor::streaming::operators::base::OperatorBase;
    use crate::executor::streaming::operators::source_operator::{
        SourceOperator, SourceOperatorKind,
    };
    use crate::executor::streaming::slot::SlotLayout;
    use graphdb_core::Value;
    use std::sync::Arc;

    fn test_layout(names: &[&str]) -> Arc<SlotLayout> {
        Arc::new(SlotLayout::from_names(
            &names.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ))
    }

    fn source_executor(rows: Vec<Vec<Value>>, col_names: Vec<&str>) -> StreamingExecutor {
        let layout = test_layout(&col_names);
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: rows,
                    current_index: 0,
                    col_names: col_names.into_iter().map(|s| s.to_string()).collect(),
                },
                layout,
            ),
        )
    }

    #[test]
    fn flatten_single_batch_selection_vector_path() {
        let rows = vec![
            vec![Value::Int(1), Value::string("a")],
            vec![Value::Int(2), Value::string("b")],
            vec![Value::Int(3), Value::string("c")],
        ];
        let mut src = source_executor(rows, vec!["id", "name"]);
        src.open().expect("open");
        let mut current_idx = 0usize;
        let mut size_to_flatten = 0usize;
        let mut saved = None;
        let mut buffered = None;
        let mut out_rows = Vec::new();
        loop {
            let chunk = flatten_next_inner(
                &mut current_idx,
                &mut size_to_flatten,
                &mut saved,
                &mut buffered,
                &mut src,
            )
            .expect("next");
            match chunk {
                Some(c) => {
                    assert_eq!(c.len(), 1);
                    out_rows.push(c.rows[0].clone());
                }
                None => break,
            }
        }
        assert_eq!(out_rows.len(), 3);
        assert_eq!(out_rows[0][0], Value::Int(1));
        assert_eq!(out_rows[1][0], Value::Int(2));
        assert_eq!(out_rows[2][0], Value::Int(3));
    }

    #[test]
    fn flatten_with_child_selection() {
        let rows = vec![
            vec![Value::Int(10)],
            vec![Value::Int(20)],
            vec![Value::Int(30)],
        ];
        let layout = test_layout(&["v"]);
        let mut chunk = DataChunk::new_with_layout(rows.clone(), layout.clone());
        chunk = chunk.with_selection(vec![0, 2]);
        assert_eq!(chunk.visible_count(), 2);
        let (sel, buffered) = prepare_flatten_buffer(chunk);
        assert_eq!(sel, vec![0, 2]);
        assert_eq!(buffered.rows.len(), 3);
        assert_eq!(buffered.visible_count(), 3);
    }

    #[test]
    fn flatten_empty_input_returns_none() {
        let mut src = source_executor(vec![], vec!["id"]);
        src.open().expect("open");
        let mut current_idx = 0usize;
        let mut size_to_flatten = 0usize;
        let mut saved = None;
        let mut buffered = None;
        let out = flatten_next_inner(
            &mut current_idx,
            &mut size_to_flatten,
            &mut saved,
            &mut buffered,
            &mut src,
        )
        .expect("next");
        assert!(out.is_none());
    }

    #[test]
    fn typed_columns_preserved() {
        let layout = test_layout(&["a", "b"]);
        let mut chunk = DataChunk::new_with_layout(
            vec![
                vec![Value::Int(1), Value::Int(10)],
                vec![Value::Int(2), Value::Int(20)],
            ],
            layout.clone(),
        );
        chunk.build_typed_columns(true);
        assert!(chunk.typed_columns.is_some());
        let (sel, buffered) = prepare_flatten_buffer(chunk);
        assert_eq!(sel.len(), 2);
        let rows = vec![
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(2), Value::Int(20)],
        ];
        let mut src = source_executor(rows, vec!["a", "b"]);
        let mut current_idx = 0usize;
        let mut size_to_flatten = sel.len();
        let mut saved = Some(sel);
        let mut buf_opt = Some(buffered);
        let out1 = flatten_next_inner(
            &mut current_idx,
            &mut size_to_flatten,
            &mut saved,
            &mut buf_opt,
            &mut src,
        )
        .expect("next")
        .expect("chunk");
        assert_eq!(out1.len(), 1);
        assert!(out1.typed_columns.is_some());
        assert_eq!(out1.typed_columns.as_ref().unwrap()[0].len(), 1);
        let out2 = flatten_next_inner(
            &mut current_idx,
            &mut size_to_flatten,
            &mut saved,
            &mut buf_opt,
            &mut src,
        )
        .expect("next")
        .expect("chunk");
        assert_eq!(out2.len(), 1);
        assert!(out2.typed_columns.is_some());
    }

    #[test]
    fn empty_selection_batch() {
        let layout = test_layout(&["v"]);
        let mut chunk = DataChunk::new_with_layout(vec![], layout.clone());
        chunk = chunk.with_selection(vec![]);
        assert_eq!(chunk.visible_count(), 0);
        let (sel, _) = prepare_flatten_buffer(chunk);
        assert_eq!(sel.len(), 0);
    }

    fn drain_batch(src: &mut StreamingExecutor, batch_size: usize) -> Vec<DataChunk> {
        let mut current_idx = 0usize;
        let mut size_to_flatten = 0usize;
        let mut saved = None;
        let mut buffered = None;
        let mut out = Vec::new();
        loop {
            let chunk = flatten_next_batch(
                &mut current_idx,
                &mut size_to_flatten,
                &mut saved,
                &mut buffered,
                src,
                batch_size,
            )
            .expect("next");
            match chunk {
                Some(c) => out.push(c),
                None => break,
            }
        }
        out
    }

    fn flat_rows(chunks: &[DataChunk]) -> Vec<Vec<Value>> {
        chunks
            .iter()
            .flat_map(|c| c.rows.clone())
            .collect::<Vec<_>>()
    }

    #[test]
    fn flatten_batch_size_two_matches_single_row_path() {
        let rows = vec![
            vec![Value::Int(1), Value::string("a")],
            vec![Value::Int(2), Value::string("b")],
            vec![Value::Int(3), Value::string("c")],
            vec![Value::Int(4), Value::string("d")],
            vec![Value::Int(5), Value::string("e")],
        ];
        let mut single = source_executor(rows.clone(), vec!["id", "name"]);
        single.open().expect("open");
        let mut current_idx = 0usize;
        let mut size_to_flatten = 0usize;
        let mut saved = None;
        let mut buffered = None;
        let mut single_rows = Vec::new();
        loop {
            let chunk = flatten_next_inner(
                &mut current_idx,
                &mut size_to_flatten,
                &mut saved,
                &mut buffered,
                &mut single,
            )
            .expect("next");
            match chunk {
                Some(c) => {
                    assert_eq!(c.len(), 1);
                    single_rows.push(c.rows[0].clone());
                }
                None => break,
            }
        }
        let mut batched = source_executor(rows, vec!["id", "name"]);
        batched.open().expect("open");
        let chunks = drain_batch(&mut batched, 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2);
        assert_eq!(chunks[1].len(), 2);
        assert_eq!(chunks[2].len(), 1);
        assert_eq!(flat_rows(&chunks), single_rows);
    }

    #[test]
    fn flatten_single_row_path_matches_full_morsel() {
        // Monotonicity across batch sizes: single-row drains must replay the
        // same rows in the same order as one full-morsel batch.
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
            vec![Value::Int(4)],
        ];
        let mut single = source_executor(rows.clone(), vec!["id"]);
        single.open().expect("open");
        let mut current_idx = 0usize;
        let mut size_to_flatten = 0usize;
        let mut saved = None;
        let mut buffered = None;
        let mut single_rows = Vec::new();
        loop {
            let chunk = flatten_next_inner(
                &mut current_idx,
                &mut size_to_flatten,
                &mut saved,
                &mut buffered,
                &mut single,
            )
            .expect("next");
            match chunk {
                Some(c) => single_rows.extend(c.rows.clone()),
                None => break,
            }
        }
        let mut batched = source_executor(rows.clone(), vec!["id"]);
        batched.open().expect("open");
        let chunks = drain_batch(&mut batched, 2048);
        assert_eq!(chunks.len(), 1);
        assert_eq!(flat_rows(&chunks), single_rows);
        assert_eq!(flat_rows(&chunks), rows);
    }

    #[test]
    fn flatten_batch_full_morsel_single_call() {
        let rows = vec![
            vec![Value::Int(1)],
            vec![Value::Int(2)],
            vec![Value::Int(3)],
        ];
        let mut src = source_executor(rows.clone(), vec!["id"]);
        src.open().expect("open");
        let chunks = drain_batch(&mut src, 2048);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 3);
        assert_eq!(
            flat_rows(&chunks),
            rows,
            "batch_size=2048 must emit the whole morsel at once"
        );
    }

    #[test]
    fn flatten_batch_typed_columns_equivalence() {
        let layout = test_layout(&["a", "b"]);
        let rows = vec![
            vec![Value::Int(1), Value::Int(10)],
            vec![Value::Int(2), Value::Int(20)],
            vec![Value::Int(3), Value::Int(30)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows.clone(), layout.clone());
        chunk.build_typed_columns(true);
        assert!(chunk.typed_columns.is_some());
        let (sel, buffered) = prepare_flatten_buffer(chunk);
        assert_eq!(sel.len(), 3);
        let mut src = source_executor(rows.clone(), vec!["a", "b"]);
        let mut current_idx = 0usize;
        let mut size_to_flatten = sel.len();
        let mut saved = Some(sel);
        let mut buf_opt = Some(buffered);
        let out = flatten_next_batch(
            &mut current_idx,
            &mut size_to_flatten,
            &mut saved,
            &mut buf_opt,
            &mut src,
            2,
        )
        .expect("next")
        .expect("chunk");
        assert_eq!(out.len(), 2);
        let typed = out.typed_columns.as_ref().expect("typed columns");
        assert_eq!(typed[0].len(), 2);
        assert_eq!(typed[1].len(), 2);
        assert_eq!(typed[0].value_at(0), Some(Value::Int(1)));
        assert_eq!(typed[0].value_at(1), Some(Value::Int(2)));
        assert_eq!(typed[1].value_at(0), Some(Value::Int(10)));
        assert_eq!(typed[1].value_at(1), Some(Value::Int(20)));
        let rest = flatten_next_batch(
            &mut current_idx,
            &mut size_to_flatten,
            &mut saved,
            &mut buf_opt,
            &mut src,
            2,
        )
        .expect("next")
        .expect("chunk");
        assert_eq!(rest.len(), 1);
        assert_eq!(
            rest.typed_columns.as_ref().expect("typed")[0].value_at(0),
            Some(Value::Int(3))
        );
        // State machine is drained after the exact row count.
        assert!(saved.is_none());
        assert!(buf_opt.is_none());
    }

    #[test]
    fn flatten_batch_empty_input_returns_none() {
        let mut src = source_executor(vec![], vec!["id"]);
        src.open().expect("open");
        let chunks = drain_batch(&mut src, 2);
        assert!(chunks.is_empty());
    }
}

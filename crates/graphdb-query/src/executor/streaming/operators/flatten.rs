use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use graphdb_core::error::QueryError;

/// Shared flatten logic used by `UnaryOperatorKind::Flatten`.
/// Production streaming uses `UnaryOperatorKind::Flatten` via `unary_operator.rs`.
/// This module provides the single `flatten_next_inner` implementation.

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
    loop {
        if let Some(chunk) = buffered_chunk.take() {
            if *current_idx < *size_to_flatten {
                let sel_vec = saved_sel_vector
                    .as_ref()
                    .expect("saved sel vector must be present");
                let sel_pos = sel_vec[*current_idx];
                *current_idx += 1;
                let remaining = *size_to_flatten - *current_idx;
                let layout = chunk.get_layout();
                let schema = chunk.schema.clone();
                let row = chunk.rows[sel_pos].clone();
                let columns = chunk.columns.as_ref().map(|cols| {
                    cols.iter()
                        .map(|col| vec![col[sel_pos].clone()])
                        .collect::<Vec<_>>()
                });
                let typed_columns = chunk.typed_columns.as_ref().map(|cols| {
                    cols.iter()
                        .map(|col| {
                            crate::executor::streaming::chunk::gather_typed_column(col, &[sel_pos])
                        })
                        .collect::<Vec<_>>()
                });
                let columnar_stats = chunk.columnar_stats.clone();
                if remaining > 0 {
                    *buffered_chunk = Some(chunk);
                } else {
                    *saved_sel_vector = None;
                    *size_to_flatten = 0;
                    *current_idx = 0;
                }
                let mut out = DataChunk::new_with_layout(vec![row], layout);
                out.schema = schema;
                out.columns = columns;
                out.typed_columns = typed_columns;
                out.columnar_stats = columnar_stats;
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
}

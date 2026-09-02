use std::sync::Arc;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;

/// Shared flatten logic used by both the standalone `FlattenOperator` and the
/// integrated `UnaryOperatorKind::Flatten`. The two operators share the same
/// selection-vector algorithm to keep the observable flatten semantics identical;

/// Flatten operator: expands an unflat column into flat rows.
///
/// SelectionVector based implementation similar to
/// `ref/ladybug/src/processor/operator/flatten.cpp`.
/// The operator does not physically copy data for the flat columns.
/// Instead it manipulates the SelectionVector of the child chunk to expose
/// one logical row at a time. Downstream operators see a flat view.
///
/// For the Rust streaming engine which currently materializes rows, the
/// implementation emulates the same observable behavior by iterating over
/// the child's selection with an index cursor.
///
/// `data_chunk_to_flatten_pos` is reserved for column-aware flattening when
/// `FactorizedTable` overflow columns are materialized; the current row-
/// granular engine flattens the chunk's selection vector instead.
#[derive(Debug)]
pub struct FlattenOperator {
    /// Slot position of the column to flatten.
    /// Derived from `group_pos` + layout resolution at open time.
    pub data_chunk_to_flatten_pos: usize,
    /// Current index inside the child batch.
    pub current_idx: usize,
    /// Total number of logical rows in the buffered child batch.
    pub size_to_flatten: usize,
    /// Saved selection vector of the buffered batch (absolute indices).
    pub saved_sel_vector: Option<Vec<usize>>,
    /// Buffered child chunk waiting to be flattened.
    pub buffered_chunk: Option<DataChunk>,
    /// Runtime for cancellation checks.
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
    /// Group position to flatten (logical factorization group).
    pub group_pos: u32,
}

impl FlattenOperator {
    pub fn new(group_pos: u32, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            data_chunk_to_flatten_pos: usize::MAX,
            current_idx: 0,
            size_to_flatten: 0,
            saved_sel_vector: None,
            buffered_chunk: None,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
            group_pos,
        }
    }

    pub fn new_with_slot(group_pos: u32, slot_pos: usize, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            data_chunk_to_flatten_pos: slot_pos,
            current_idx: 0,
            size_to_flatten: 0,
            saved_sel_vector: None,
            buffered_chunk: None,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
            group_pos,
        }
    }

    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        self.current_idx = 0;
        self.size_to_flatten = 0;
        self.saved_sel_vector = None;
        self.buffered_chunk = None;
        // Resolve slot position lazily on first batch if not set.
        // Keep usize::MAX as sentinel for "resolve from layout on first chunk".
        input.open()?;
        Ok(())
    }

    fn prepare_buffered_chunk(&mut self, chunk: DataChunk) -> DataChunk {
        let (sel, buffered) = prepare_flatten_buffer(chunk);
        self.saved_sel_vector = Some(sel.clone());
        self.size_to_flatten = sel.len();
        self.current_idx = 0;
        buffered
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        flatten_next_inner(
            &mut self.current_idx,
            &mut self.size_to_flatten,
            &mut self.saved_sel_vector,
            &mut self.buffered_chunk,
            input,
        )
    }

    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        self.current_idx = 0;
        self.size_to_flatten = 0;
        self.saved_sel_vector = None;
        self.buffered_chunk = None;
        input.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        self.buffered_chunk = None;
        self.saved_sel_vector = None;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }
}

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
                if remaining > 0 {
                    *buffered_chunk = Some(chunk);
                } else {
                    *saved_sel_vector = None;
                    *size_to_flatten = 0;
                    *current_idx = 0;
                }
                let mut out = DataChunk::new_with_layout(vec![row], layout);
                out.schema = schema;
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
        let sel = child_chunk.visible_indices();
        *saved_sel_vector = Some(sel.clone());
        *size_to_flatten = sel.len();
        *current_idx = 0;
        let mut buffered = child_chunk;
        let _ = buffered.take_selection();
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
        // Chunk with 3 rows, no selection -> flatten should emit 3 single-row chunks.
        let rows = vec![
            vec![Value::Int(1), Value::string("a")],
            vec![Value::Int(2), Value::string("b")],
            vec![Value::Int(3), Value::string("c")],
        ];
        let mut src = source_executor(rows, vec!["id", "name"]);
        let layout = test_layout(&["id", "name"]);
        let mut flatten = FlattenOperator::new(1, layout);

        flatten.open(&mut src).expect("open");
        let mut out_rows = Vec::new();
        while let Some(chunk) = flatten.next(&mut src).expect("next") {
            assert_eq!(chunk.len(), 1);
            out_rows.push(chunk.rows[0].clone());
        }
        assert_eq!(out_rows.len(), 3);
        assert_eq!(out_rows[0][0], Value::Int(1));
        assert_eq!(out_rows[1][0], Value::Int(2));
        assert_eq!(out_rows[2][0], Value::Int(3));
    }

    #[test]
    fn flatten_with_child_selection() {
        // Child emits 3 rows but predicate filter keeps only indices [0,2].
        // Flatten should emit only those 2.
        let rows = vec![
            vec![Value::Int(10)],
            vec![Value::Int(20)],
            vec![Value::Int(30)],
        ];
        let layout = test_layout(&["v"]);
        let mut chunk = DataChunk::new_with_layout(rows.clone(), layout.clone());
        chunk = chunk.with_selection(vec![0, 2]);
        assert_eq!(chunk.visible_count(), 2);

        let mut src = StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: chunk.rows.clone(),
                    current_index: 0,
                    col_names: vec!["v".to_string()],
                },
                layout.clone(),
            ),
        );
        // Inject filtered chunk via custom source that yields our pre-filtered chunk.
        // For simplicity, test the prepare_buffered_chunk directly.
        let mut flatten = FlattenOperator::new(0, layout);
        let buffered = flatten.prepare_buffered_chunk(chunk);
        assert_eq!(flatten.size_to_flatten, 2);
        assert_eq!(flatten.saved_sel_vector, Some(vec![0, 2]));
        assert_eq!(buffered.rows.len(), 3);
    }

    #[test]
    fn flatten_empty_input_returns_none() {
        let mut src = source_executor(vec![], vec!["id"]);
        let layout = test_layout(&["id"]);
        let mut flatten = FlattenOperator::new(0, layout);
        flatten.open(&mut src).expect("open");
        let out = flatten.next(&mut src).expect("next");
        assert!(out.is_none());
    }
}

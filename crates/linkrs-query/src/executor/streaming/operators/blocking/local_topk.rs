//! Partition-local distinct TopK state.
//!
//! The operator has two modes chosen from the first input chunk. A typed
//! I64 key column is truncated to the k best keys in O(k) space. Any other
//! shape switches to pass-through: every visible row is emitted unchanged
//! and the global Distinct/TopN above the exchange remains authoritative.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::{bitmap_is_valid, DataChunk, TypedColumn};
use crate::executor::streaming::executor::SortDirection;

use super::helpers::{emit_batch_slice, BlockingContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Undecided,
    Typed,
    PassThrough,
}

#[derive(Debug)]
pub struct LocalTopKDistinctState {
    limit: usize,
    direction: SortDirection,
    key_name: String,
    mode: Mode,
    heap: BinaryHeap<i64>,
    reversed_heap: BinaryHeap<Reverse<i64>>,
    seen_keys: HashSet<i64>,
    saw_null: bool,
    batch: MaterializedBatch,
    emitted_offset: usize,
    col_names: Vec<String>,
}

impl LocalTopKDistinctState {
    pub fn new(limit: u32, direction: SortDirection, key_name: String) -> Self {
        Self {
            limit: limit as usize,
            direction,
            key_name,
            mode: Mode::Undecided,
            heap: BinaryHeap::new(),
            reversed_heap: BinaryHeap::new(),
            seen_keys: HashSet::new(),
            saw_null: false,
            batch: MaterializedBatch::new(0, 0),
            emitted_offset: 0,
            col_names: Vec::new(),
        }
    }

    /// Returns true when the key is retained and needs memory accounting.
    fn insert_key(&mut self, key: i64) -> bool {
        if self.seen_keys.contains(&key) {
            return false;
        }
        match self.direction {
            SortDirection::Ascending => {
                if self.heap.len() < self.limit {
                    self.heap.push(key);
                    self.seen_keys.insert(key);
                    return true;
                }
                if let Some(&worst) = self.heap.peek() {
                    if key < worst {
                        self.heap.pop();
                        self.seen_keys.remove(&worst);
                        self.heap.push(key);
                        self.seen_keys.insert(key);
                    }
                }
            }
            SortDirection::Descending => {
                if self.reversed_heap.len() < self.limit {
                    self.reversed_heap.push(Reverse(key));
                    self.seen_keys.insert(key);
                    return true;
                }
                if let Some(&Reverse(worst)) = self.reversed_heap.peek() {
                    if key > worst {
                        self.reversed_heap.pop();
                        self.seen_keys.remove(&worst);
                        self.reversed_heap.push(Reverse(key));
                        self.seen_keys.insert(key);
                    }
                }
            }
        }
        false
    }

    fn accept_typed_keys(
        &mut self,
        keys: Vec<Option<i64>>,
        memory_tracker: &mut MemoryTracker,
    ) -> Result<(), QueryError> {
        for key in keys {
            match key {
                Some(key) => {
                    if self.insert_key(key) {
                        memory_tracker.try_reserve(32)?;
                    }
                }
                None => {
                    if !self.saw_null {
                        self.saw_null = true;
                        memory_tracker.try_reserve(32)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn emit_typed_batch(&mut self) {
        let mut keys: Vec<i64> = match self.direction {
            SortDirection::Ascending => self.heap.clone().into_vec(),
            SortDirection::Descending => self.reversed_heap.iter().map(|Reverse(k)| *k).collect(),
        };
        match self.direction {
            SortDirection::Ascending => keys.sort_unstable(),
            SortDirection::Descending => keys.sort_unstable_by(|a, b| b.cmp(a)),
        }
        self.batch.clear();
        // The global comparator puts NULL after numbers before reversing the
        // direction, so NULL is last for ASC and first for DESC. It is kept
        // as one deduplicated key and can evict a non-null candidate in DESC.
        if self.saw_null && self.direction == SortDirection::Descending {
            self.batch.append_row(vec![Value::Null(NullType::Null)]);
            keys.truncate(self.limit.saturating_sub(1));
        }
        for key in keys {
            self.batch.append_row(vec![Value::BigInt(key)]);
        }
        if self.saw_null
            && self.direction == SortDirection::Ascending
            && self.batch.num_rows() < self.limit
        {
            self.batch.append_row(vec![Value::Null(NullType::Null)]);
        }
    }

    fn typed_keys(chunk: &mut DataChunk, key_name: &str) -> Option<Vec<Option<i64>>> {
        let slot = chunk.get_layout().slot_id(key_name)?;
        chunk.ensure_typed_columns();
        let (values, validity) = match chunk.typed_column(slot)? {
            TypedColumn::I64(values) => (values, None),
            TypedColumn::NullableI64(values, bitmap) => (values, Some(bitmap)),
            _ => return None,
        };
        let len = if chunk.rows.is_empty() {
            values.len()
        } else {
            chunk.rows.len()
        };
        let indices = match chunk.selection() {
            Some(indices) => indices.to_vec(),
            None => (0..len).collect(),
        };
        if indices.iter().any(|&idx| idx >= values.len()) {
            return None;
        }
        Some(
            indices
                .into_iter()
                .map(|idx| match validity {
                    Some(bitmap) if !bitmap_is_valid(bitmap, idx) => None,
                    _ => Some(values[idx]),
                })
                .collect(),
        )
    }
}

pub(super) fn next(
    memory_tracker: &mut MemoryTracker,
    state: &mut LocalTopKDistinctState,
    ctx: &BlockingContext<'_>,
    input: &mut crate::executor::streaming::executor::StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if state.emitted_offset < state.batch.num_rows() {
        return Ok(Some(emit_batch_slice(
            &state.batch,
            &mut state.emitted_offset,
            ctx,
        )));
    }
    if state.emitted_offset > 0 {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        if let Some(rt) = ctx.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if state.col_names.is_empty() {
            state.col_names = chunk.col_names();
        }
        let keys = LocalTopKDistinctState::typed_keys(&mut chunk, &state.key_name);
        if state.mode == Mode::Undecided {
            state.mode = if keys.is_some() {
                Mode::Typed
            } else {
                Mode::PassThrough
            };
        }

        match state.mode {
            Mode::Typed => {
                let keys = keys.ok_or_else(|| {
                    QueryError::execution(format!(
                        "LocalTopKDistinct typed key '{}' changed after the first chunk",
                        state.key_name
                    ))
                })?;
                state.accept_typed_keys(keys, memory_tracker)?;
            }
            Mode::PassThrough => {
                let slot = chunk
                    .get_layout()
                    .slot_id(&state.key_name)
                    .unwrap_or_else(|| chunk.get_layout().len().saturating_sub(1));
                chunk.materialize_typed_rows();
                for row in chunk.visible_rows() {
                    let value = row
                        .get(slot)
                        .cloned()
                        .unwrap_or(Value::Null(linkrs_core::value::NullType::Null));
                    memory_tracker.try_reserve(32)?;
                    state.batch.append_row(vec![value]);
                }
            }
            Mode::Undecided => unreachable!("mode is decided for every input chunk"),
        }
    }

    if state.mode == Mode::Typed {
        state.emit_typed_batch();
    }
    if !state.col_names.is_empty() {
        state
            .batch
            .set_schema_names(Arc::from(state.col_names.clone().into_boxed_slice()));
    }
    state.emitted_offset = 0;
    if state.batch.is_empty() {
        return Ok(None);
    }
    Ok(Some(emit_batch_slice(
        &state.batch,
        &mut state.emitted_offset,
        ctx,
    )))
}

pub(super) fn close(state: &mut Option<LocalTopKDistinctState>) {
    *state = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascending_keeps_smallest_unique_keys() {
        let mut state = LocalTopKDistinctState::new(3, SortDirection::Ascending, "k".into());
        for key in [5, 1, 5, 9, 2, 7] {
            let _ = state.insert_key(key);
        }
        let mut keys: Vec<_> = state.heap.into_vec();
        keys.sort_unstable();
        assert_eq!(keys, vec![1, 2, 5]);
    }

    #[test]
    fn descending_keeps_largest_unique_keys() {
        let mut state = LocalTopKDistinctState::new(2, SortDirection::Descending, "k".into());
        for key in [5, 1, 9, 9, 7] {
            let _ = state.insert_key(key);
        }
        let mut keys: Vec<_> = state.reversed_heap.iter().map(|Reverse(k)| *k).collect();
        keys.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(keys, vec![9, 7]);
    }

    #[test]
    fn zero_limit_keeps_no_keys() {
        let mut state = LocalTopKDistinctState::new(0, SortDirection::Ascending, "k".into());
        assert!(!state.insert_key(1));
        assert!(state.heap.is_empty());
        assert!(state.seen_keys.is_empty());
    }

    #[test]
    fn limit_larger_than_input_keeps_every_unique_key() {
        let mut state = LocalTopKDistinctState::new(10, SortDirection::Ascending, "k".into());
        for key in [5, 1, 5, 9, 2] {
            let _ = state.insert_key(key);
        }
        let mut keys: Vec<_> = state.heap.clone().into_vec();
        keys.sort_unstable();
        assert_eq!(keys, vec![1, 2, 5, 9]);
    }

    #[test]
    fn typed_keys_gather_rowless_and_selected_values() {
        use crate::executor::streaming::slot::SlotLayout;

        let layout = Arc::new(SlotLayout::from_names(&["k".to_string()]));
        let mut rowless = DataChunk::new_with_layout(Vec::new(), Arc::clone(&layout));
        rowless.typed_columns = Some(vec![TypedColumn::I64(vec![5, 1, 5, 9])]);
        assert_eq!(
            LocalTopKDistinctState::typed_keys(&mut rowless, "k"),
            Some(vec![Some(5), Some(1), Some(5), Some(9)])
        );

        let mut selected = DataChunk::new_with_layout(
            vec![
                vec![Value::BigInt(5)],
                vec![Value::BigInt(1)],
                vec![Value::BigInt(9)],
            ],
            layout,
        )
        .with_selection(vec![0, 2]);
        selected.typed_columns = Some(vec![TypedColumn::I64(vec![5, 1, 9])]);
        assert_eq!(
            LocalTopKDistinctState::typed_keys(&mut selected, "k"),
            Some(vec![Some(5), Some(9)])
        );
    }

    #[test]
    fn typed_keys_read_nullable_i64_validity() {
        use crate::executor::streaming::slot::SlotLayout;

        let layout = Arc::new(SlotLayout::from_names(&["k".to_string()]));
        let mut chunk = DataChunk::new_with_layout(Vec::new(), Arc::clone(&layout));
        chunk.typed_columns = Some(vec![TypedColumn::NullableI64(vec![5, 1, 9], vec![0b101])]);
        assert_eq!(
            LocalTopKDistinctState::typed_keys(&mut chunk, "k"),
            Some(vec![Some(5), None, Some(9)])
        );
    }

    #[test]
    fn nullable_keys_follow_global_null_placement() {
        use crate::executor::base::MemoryBudget;

        let mut tracker = MemoryTracker::new(MemoryBudget::default_budget());
        let mut ascending = LocalTopKDistinctState::new(3, SortDirection::Ascending, "k".into());
        ascending
            .accept_typed_keys(vec![Some(5), None, Some(1), Some(9)], &mut tracker)
            .expect("accept nullable keys");
        ascending.emit_typed_batch();
        let rows: Vec<Vec<Value>> = ascending.batch.rows().collect();
        assert_eq!(
            rows,
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(5)],
                vec![Value::BigInt(9)],
            ]
        );

        let mut ascending_with_room =
            LocalTopKDistinctState::new(3, SortDirection::Ascending, "k".into());
        ascending_with_room
            .accept_typed_keys(vec![Some(5), None, Some(1), Some(5)], &mut tracker)
            .expect("accept nullable keys");
        ascending_with_room.emit_typed_batch();
        let rows: Vec<Vec<Value>> = ascending_with_room.batch.rows().collect();
        assert_eq!(
            rows,
            vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(5)],
                vec![Value::Null(NullType::Null)],
            ]
        );

        let mut descending = LocalTopKDistinctState::new(3, SortDirection::Descending, "k".into());
        descending
            .accept_typed_keys(vec![Some(5), None, Some(9), Some(7)], &mut tracker)
            .expect("accept nullable keys");
        descending.emit_typed_batch();
        let rows: Vec<Vec<Value>> = descending.batch.rows().collect();
        assert_eq!(
            rows,
            vec![
                vec![Value::Null(NullType::Null)],
                vec![Value::BigInt(9)],
                vec![Value::BigInt(7)],
            ]
        );
    }

    #[test]
    fn pass_through_preserves_null_and_non_i64_values() {
        use crate::executor::base::MemoryBudget;
        use crate::executor::streaming::executor::StreamingExecutor;
        use crate::executor::streaming::operators::source_operator::{
            OperatorConfig, SourceOperator, SourceOperatorKind,
        };
        use crate::executor::streaming::slot::SlotLayout;
        use crate::executor::streaming::OperatorBase;
        use linkrs_core::value::NullType;

        let layout = Arc::new(SlotLayout::from_names(&["k".to_string()]));
        let input_rows = vec![
            vec![Value::BigInt(5)],
            vec![Value::Null(NullType::Null)],
            vec![Value::string("abc")],
            vec![Value::BigInt(5)],
        ];
        let mut source = StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: input_rows.clone(),
                    current_index: 0,
                    col_names: vec!["k".to_string()],
                },
                Arc::clone(&layout),
            ),
        );
        source.open().expect("open source");

        let runtime = None;
        let config = OperatorConfig::default();
        let ctx = BlockingContext {
            runtime: &runtime,
            output_layout: &layout,
            config: &config,
        };
        let mut tracker = MemoryTracker::new(MemoryBudget::default_budget());
        let mut state = LocalTopKDistinctState::new(2, SortDirection::Ascending, "k".into());
        let emitted = next(&mut tracker, &mut state, &ctx, &mut source)
            .expect("pass through")
            .expect("emitted chunk");
        let actual: Vec<_> = emitted.visible_rows().cloned().collect();
        assert_eq!(actual, input_rows);
        assert_eq!(state.mode, Mode::PassThrough);
    }
}

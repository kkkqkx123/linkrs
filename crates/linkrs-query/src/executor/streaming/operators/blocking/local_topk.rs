//! Partition-local distinct TopK state.
//!
//! The operator keeps at most `k` fixed-width keys for the narrow typed
//! shape and degrades to an exact full-row distinct set for anything it
//! cannot compare as `i64`. Degraded output is unsorted and unbounded; the
//! global Distinct/TopN above the exchange remains authoritative.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::Arc;

use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::error::QueryError;
use linkrs_core::Value;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::{DataChunk, TypedColumn};
use crate::executor::streaming::executor::SortDirection;

use super::helpers::{emit_batch_slice, BlockingContext};

#[derive(Debug)]
pub struct LocalTopKDistinctState {
    limit: usize,
    direction: SortDirection,
    key_name: String,
    heap: BinaryHeap<i64>,
    reversed_heap: BinaryHeap<Reverse<i64>>,
    seen_keys: HashSet<i64>,
    seen_values: HashSet<Vec<Value>>,
    degraded: bool,
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
            heap: BinaryHeap::new(),
            reversed_heap: BinaryHeap::new(),
            seen_keys: HashSet::new(),
            seen_values: HashSet::new(),
            degraded: false,
            batch: MaterializedBatch::new(0, 0),
            emitted_offset: 0,
            col_names: Vec::new(),
        }
    }

    fn insert_key(&mut self, key: i64) -> bool {
        let is_new = self.seen_values.insert(vec![Value::BigInt(key)]);
        if self.degraded {
            return is_new;
        }
        if self.seen_keys.contains(&key) {
            return is_new;
        }
        match self.direction {
            SortDirection::Ascending => {
                if self.heap.len() < self.limit {
                    self.heap.push(key);
                    self.seen_keys.insert(key);
                } else if let Some(&worst) = self.heap.peek() {
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
                } else if let Some(&Reverse(worst)) = self.reversed_heap.peek() {
                    if key > worst {
                        self.reversed_heap.pop();
                        self.seen_keys.remove(&worst);
                        self.reversed_heap.push(Reverse(key));
                        self.seen_keys.insert(key);
                    }
                }
            }
        }
        is_new
    }

    fn degrade(&mut self) {
        if self.degraded {
            return;
        }
        self.degraded = true;
        self.seen_keys.clear();
        self.heap.clear();
        self.reversed_heap.clear();
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
        let slot = chunk
            .get_layout()
            .slot_id(&state.key_name)
            .unwrap_or_else(|| chunk.get_layout().len().saturating_sub(1));
        let typed_keys = if chunk.rows.is_empty()
            && chunk.selection().is_none()
            && chunk.multiplicity() == 1
            && slot < chunk.get_layout().len()
        {
            match chunk.typed_column(slot) {
                Some(TypedColumn::I64(values)) => Some(values.clone()),
                _ => None,
            }
        } else {
            None
        };

        if let Some(keys) = typed_keys {
            for key in keys {
                if state.insert_key(key) {
                    memory_tracker.try_reserve(32)?;
                }
            }
        } else {
            state.degrade();
            chunk.materialize_typed_rows();
            for row in chunk.visible_rows() {
                let value = row
                    .get(slot)
                    .cloned()
                    .unwrap_or(Value::Null(linkrs_core::value::NullType::Null));
                if state.seen_values.insert(vec![value]) {
                    memory_tracker.try_reserve(32)?;
                }
            }
        }
    }

    let mut keys: Vec<i64> = if state.degraded {
        Vec::new()
    } else {
        match state.direction {
            SortDirection::Ascending => state.heap.clone().into_vec(),
            SortDirection::Descending => state
                .reversed_heap
                .iter()
                .map(|Reverse(key)| *key)
                .collect(),
        }
    };
    if state.degraded {
        let mut rows: Vec<Vec<Value>> = state.seen_values.iter().cloned().collect();
        // Global TopN is the authority on order; a stable local order only
        // keeps the exchange deterministic for repeated runs.
        rows.sort_by(|a, b| format!("{:?}", a[0]).cmp(&format!("{:?}", b[0])));
        state.batch.clear();
        for row in rows {
            state.batch.append_row(row);
        }
    } else {
        match state.direction {
            SortDirection::Ascending => keys.sort_unstable(),
            SortDirection::Descending => keys.sort_unstable_by(|a, b| b.cmp(a)),
        }
        state.batch.clear();
        for key in keys {
            state.batch.append_row(vec![Value::BigInt(key)]);
        }
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
    fn degradation_keeps_all_seen_keys() {
        let mut state = LocalTopKDistinctState::new(1, SortDirection::Ascending, "k".into());
        let _ = state.insert_key(9);
        let _ = state.insert_key(1);
        state.degrade();
        let _ = state.insert_key(7);
        assert_eq!(state.seen_values.len(), 3);
        assert!(state.seen_values.contains(&vec![Value::BigInt(9)]));
        assert!(state.seen_values.contains(&vec![Value::BigInt(7)]));
    }

    #[test]
    fn zero_limit_emits_nothing() {
        let mut state = LocalTopKDistinctState::new(0, SortDirection::Ascending, "k".into());
        let _ = state.insert_key(1);
        assert!(state.heap.is_empty());
        assert!(state.seen_keys.is_empty());
    }
}

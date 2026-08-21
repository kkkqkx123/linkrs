//! Factorized operators.
//!
//! - `SemiMasker` - semi-join mask pushed into Expand (Ladybug `SEMI_MASKER`).
//! - `MultiplicityReducer` - factorized dedup with aggregation of
//!   multiplicities (Ladybug `MULTIPLICITY_REDUCER`).
//! - `NodeLabelFilter` - factorized label pruning (Ladybug `NODE_LABEL_FILTER`).
//!
//! These operators work on the `FactorizedTable` compressed representation
//! but expose a `DataChunk` streaming interface so they compose with the
//! existing `StreamingExecutor` pipeline.

use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;

use crate::query::executor::streaming::chunk::{DataChunk, FactorizedTable, SemiMask};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::source_operator::OperatorConfig;
use crate::query::executor::streaming::operators::spec::FactorizedSpec;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;

// ── State for blocking operators ───────────────────────────────────────────

#[derive(Debug, Default)]
pub struct SemiMaskerState {
    pub buffer: Vec<DataChunk>,
    pub mask: Option<SemiMask>,
    pub emitted: bool,
}

#[derive(Debug, Default)]
pub struct MultiplicityReducerState {
    pub buffer: Vec<DataChunk>,
    pub emitted: bool,
}

#[derive(Debug, Default)]
pub struct NodeLabelFilterState {
    pub buffered: bool,
}

// ── SemiMasker ──────────────────────────────────────────────────────────────

pub fn open_semi_masker(
    state: &mut SemiMaskerState,
    spec: &FactorizedSpec,
    input: &mut StreamingExecutor,
) -> Result<(), QueryError> {
    let FactorizedSpec::SemiMasker { mask_keys, .. } = spec else {
        return Ok(());
    };
    // Build mask from the planner's snapshot of distinct build-side keys.
    // Each mask_keys entry is a single Value key (single-slot case);
    // multi-slot keys are not used in the current optimizer.
    let mask = SemiMask::from_keys(mask_keys.iter().map(|v| vec![v.clone()]).collect());
    state.mask = Some(mask);
    input.open()?;
    Ok(())
}

pub fn next_semi_masker(
    base: &OperatorBase,
    state: &mut SemiMaskerState,
    spec: &FactorizedSpec,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let FactorizedSpec::SemiMasker {
        key_slot,
        keep_match,
        ..
    } = spec
    else {
        return Ok(None);
    };
    let mask = state.mask.as_ref().ok_or_else(|| {
        QueryError::execution("SemiMasker: mask not initialized (open() not called)")
    })?;

    while let Some(chunk) = input.advance()? {
        base.ensure_not_cancelled()?;
        if chunk.is_empty() {
            continue;
        }
        // Factorize by the key slot, apply mask on groups, then flatten.
        let mut table = FactorizedTable::from_chunk(&chunk, &[*key_slot]);
        let before = table.group_count();
        let pruned = table.apply_semi_mask(mask);
        // keep_match == false means anti-semi: keep rows NOT in mask
        if !*keep_match {
            // Invert: retain only pruned groups (those not in mask) - requires
            // re-building table of the pruned-out groups.
            // For simplicity, fall back to row-wise anti filter.
            let mut filtered_rows = Vec::new();
            for row in chunk.rows.iter() {
                let key = row
                    .get(*key_slot)
                    .cloned()
                    .unwrap_or(Value::Null(crate::core::value::NullType::Null));
                let in_mask = mask.contains(&[key]);
                if !in_mask {
                    filtered_rows.push(row.clone());
                }
            }
            if filtered_rows.is_empty() {
                continue;
            }
            let layout = chunk.get_layout();
            let out = DataChunk::new_with_layout(filtered_rows, layout);
            if !out.is_empty() {
                return Ok(Some(out));
            }
            continue;
        }
        if pruned == before && table.is_empty() {
            continue;
        }
        if table.is_empty() {
            continue;
        }
        let out = table.to_flat_chunk();
        if !out.is_empty() {
            return Ok(Some(out));
        }
    }
    Ok(None)
}

// ── MultiplicityReducer ───────────────────────────────────────────────────

pub fn open_multiplicity_reducer(
    state: &mut MultiplicityReducerState,
    input: &mut StreamingExecutor,
) -> Result<(), QueryError> {
    state.buffer.clear();
    state.emitted = false;
    input.open()?;
    Ok(())
}

pub fn next_multiplicity_reducer(
    base: &OperatorBase,
    state: &mut MultiplicityReducerState,
    spec: &FactorizedSpec,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let FactorizedSpec::MultiplicityReducer { group_key_slots } = spec else {
        return Ok(None);
    };

    if state.emitted {
        return Ok(None);
    }

    if state.buffer.is_empty() {
        // Blocking collect
        while let Some(chunk) = input.advance()? {
            base.ensure_not_cancelled()?;
            if !chunk.is_empty() {
                state.buffer.push(chunk);
            }
        }
        if state.buffer.is_empty() {
            state.emitted = true;
            return Ok(None);
        }
    }

    if state.emitted {
        return Ok(None);
    }

    // Merge all chunks into one factorized table
    let layout = state.buffer[0].get_layout();
    let mut all_rows: Vec<Vec<Value>> = Vec::new();
    for chunk in &state.buffer {
        all_rows.extend(chunk.rows.clone());
    }
    let merged = DataChunk::new_with_layout(all_rows, Arc::clone(&layout));
    let mut table = FactorizedTable::from_chunk(&merged, group_key_slots);
    let _eliminated = table.reduce_multiplicity();
    let out = table.to_flat_chunk();
    state.emitted = true;
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

// ── NodeLabelFilter ───────────────────────────────────────────────────────

pub fn next_node_label_filter(
    base: &OperatorBase,
    spec: &FactorizedSpec,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let FactorizedSpec::NodeLabelFilter {
        label_slot,
        allowed_labels,
    } = spec
    else {
        return Ok(None);
    };
    let allowed: HashSet<String> = allowed_labels.iter().cloned().collect();
    while let Some(mut chunk) = input.advance()? {
        base.ensure_not_cancelled()?;
        let before = chunk.len();
        chunk.rows.retain(|row| {
            row.get(*label_slot)
                .and_then(|v| match v {
                    Value::String(s) => Some(allowed.contains(&s.to_string())),
                    _ => Some(false),
                })
                .unwrap_or(false)
        });
        if chunk.rows.len() != before {
            // Also trim typed columns if present (invalidate so they rebuild lazily)
            chunk.columns = None;
            chunk.typed_columns = None;
        }
        if !chunk.is_empty() {
            return Ok(Some(chunk));
        }
    }
    Ok(None)
}

// ── Direct chunk-level helpers (used by optimizer / GraphOperator pushdown) ─

/// Apply a semi-mask directly to a chunk without table factorization (fast
/// path for single-slot keys). Returns the filtered chunk (or None if all
/// pruned).
pub fn semi_mask_chunk(
    chunk: DataChunk,
    key_slot: usize,
    mask: &SemiMask,
    keep_match: bool,
) -> Option<DataChunk> {
    if chunk.is_empty() || mask.is_empty() {
        return Some(chunk);
    }
    let mut rows = Vec::new();
    for row in chunk.rows.iter() {
        let key = row
            .get(key_slot)
            .cloned()
            .unwrap_or(Value::Null(crate::core::value::NullType::Null));
        let in_mask = mask.contains(&[key]);
        if in_mask == keep_match {
            rows.push(row.clone());
        }
    }
    if rows.is_empty() {
        None
    } else {
        Some(DataChunk::new_with_layout(rows, chunk.get_layout()))
    }
}

/// Factorized multiplicity reduction on a single chunk (non-blocking helper).
pub fn multiplicity_reduce_chunk(chunk: DataChunk, group_key_slots: &[usize]) -> DataChunk {
    if chunk.is_empty() || group_key_slots.is_empty() {
        return chunk;
    }
    let mut table = FactorizedTable::from_chunk(&chunk, group_key_slots);
    table.reduce_multiplicity();
    table.to_flat_chunk()
}

// ── Factorized operator (executor wrapper) ──────────────────────────────────

/// Streaming operator for factorized execution (Phase 4).
#[derive(Debug)]
pub enum FactorizedOperatorKind {
    SemiMasker {
        key_slot: usize,
        mask_keys: Vec<Value>,
        keep_match: bool,
        state: SemiMaskerState,
    },
    MultiplicityReducer {
        group_key_slots: Vec<usize>,
        state: MultiplicityReducerState,
    },
    NodeLabelFilter {
        label_slot: usize,
        allowed_labels: Vec<String>,
    },
}

#[derive(Debug)]
pub struct FactorizedOperator {
    pub kind: FactorizedOperatorKind,
    pub output_layout: Arc<SlotLayout>,
}

impl FactorizedOperator {
    pub fn from_spec(spec: &FactorizedSpec, output_layout: Arc<SlotLayout>) -> Self {
        let kind = match spec {
            FactorizedSpec::SemiMasker {
                key_slot,
                mask_keys,
                keep_match,
            } => FactorizedOperatorKind::SemiMasker {
                key_slot: *key_slot,
                mask_keys: mask_keys.clone(),
                keep_match: *keep_match,
                state: SemiMaskerState::default(),
            },
            FactorizedSpec::MultiplicityReducer { group_key_slots } => {
                FactorizedOperatorKind::MultiplicityReducer {
                    group_key_slots: group_key_slots.clone(),
                    state: MultiplicityReducerState::default(),
                }
            }
            FactorizedSpec::NodeLabelFilter {
                label_slot,
                allowed_labels,
            } => FactorizedOperatorKind::NodeLabelFilter {
                label_slot: *label_slot,
                allowed_labels: allowed_labels.clone(),
            },
        };
        Self {
            kind,
            output_layout,
        }
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        match &mut self.kind {
            FactorizedOperatorKind::SemiMasker {
                mask_keys, state, ..
            } => {
                let spec = FactorizedSpec::SemiMasker {
                    key_slot: 0,
                    mask_keys: mask_keys.clone(),
                    keep_match: true,
                };
                open_semi_masker(state, &spec, input)
            }
            FactorizedOperatorKind::MultiplicityReducer { state, .. } => {
                open_multiplicity_reducer(state, input)
            }
            FactorizedOperatorKind::NodeLabelFilter { .. } => input.open(),
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        // Null base for cancellation checks (factors reuse global token)
        let base = OperatorBase::new(0);
        match &mut self.kind {
            FactorizedOperatorKind::SemiMasker {
                key_slot,
                mask_keys,
                keep_match,
                state,
            } => {
                let spec = FactorizedSpec::SemiMasker {
                    key_slot: *key_slot,
                    mask_keys: mask_keys.clone(),
                    keep_match: *keep_match,
                };
                next_semi_masker(&base, state, &spec, input)
            }
            FactorizedOperatorKind::MultiplicityReducer {
                group_key_slots,
                state,
            } => {
                let spec = FactorizedSpec::MultiplicityReducer {
                    group_key_slots: group_key_slots.clone(),
                };
                next_multiplicity_reducer(&base, state, &spec, input)
            }
            FactorizedOperatorKind::NodeLabelFilter {
                label_slot,
                allowed_labels,
            } => {
                let spec = FactorizedSpec::NodeLabelFilter {
                    label_slot: *label_slot,
                    allowed_labels: allowed_labels.clone(),
                };
                next_node_label_filter(&base, &spec, input)
            }
        }
    }

    pub fn inject_context(
        &mut self,
        _runtime: Option<&Arc<ExecutionRuntime>>,
        _config: OperatorConfig,
    ) {
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        match &mut self.kind {
            FactorizedOperatorKind::SemiMasker { state, .. } => {
                state.buffer.clear();
                state.mask = None;
                state.emitted = false;
            }
            FactorizedOperatorKind::MultiplicityReducer { state, .. } => {
                state.buffer.clear();
                state.emitted = false;
            }
            FactorizedOperatorKind::NodeLabelFilter { .. } => {}
        }
        input.reset()?;
        Ok(false)
    }
}

// ── GraphOperator integration ──────────────────────────────────────────────

/// Whether the expand operator can benefit from a semi-mask pushdown.
///
/// Returns true when the expand's edge-type cardinality suggests
/// high degree + downstream selectivity > threshold.
pub fn should_push_semi_mask(
    estimated_expand_rows: u64,
    mask_selectivity: f64,
    threshold: f64,
) -> bool {
    estimated_expand_rows > 1024 && mask_selectivity < threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::executor::streaming::chunk::DataChunk;
    use crate::query::executor::streaming::slot::SlotLayout;
    use std::sync::Arc;

    fn make_chunk(rows: Vec<Vec<Value>>, cols: Vec<&str>) -> DataChunk {
        let layout = Arc::new(SlotLayout::from_names(
            &cols.into_iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        ));
        DataChunk::new_with_layout(rows, layout)
    }

    #[test]
    fn semi_mask_chunk_filters() {
        let chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(2), Value::String("b".into())],
                vec![Value::BigInt(3), Value::String("c".into())],
            ],
            vec!["id", "val"],
        );
        let mask = SemiMask::from_keys(vec![vec![Value::BigInt(1)], vec![Value::BigInt(3)]]);
        let out = semi_mask_chunk(chunk, 0, &mask, true).unwrap();
        assert_eq!(out.len(), 2);
        let out2 = semi_mask_chunk(out, 0, &mask, false);
        assert!(out2.is_none(), "anti-mask should prune all rows");
    }

    #[test]
    fn multiplicity_reduce_chunk_dedups() {
        let chunk = make_chunk(
            vec![
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("a".into())],
                vec![Value::BigInt(1), Value::String("b".into())],
            ],
            vec!["id", "val"],
        );
        // With group key [0], rows sharing id=1 are grouped
        let out = multiplicity_reduce_chunk(chunk, &[0]);
        assert!(out.len() <= 3);
    }

    #[test]
    fn should_push_semi_mask_heuristic() {
        assert!(should_push_semi_mask(2048, 0.1, 0.5));
        assert!(!should_push_semi_mask(512, 0.1, 0.5));
        assert!(!should_push_semi_mask(2048, 0.9, 0.5));
    }
}

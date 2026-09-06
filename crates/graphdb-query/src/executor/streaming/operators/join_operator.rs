use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::context::BorrowedRowContext;
use crate::executor::streaming::executor::FullOuterJoinPhase;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

mod cross_semi_join;
pub mod grace_join;
mod hash_join;
mod merge_join;
mod nested_loop_join;

fn build_combined_names(
    left_col_names: &[String],
    right_col_names: &[String],
    fallback_right_width: usize,
) -> Vec<String> {
    let mut names = left_col_names.to_vec();
    if !right_col_names.is_empty() {
        names.extend_from_slice(right_col_names);
    } else {
        for i in 0..fallback_right_width {
            names.push(format!("right_{}", i));
        }
    }
    names
}

/// Specialized hash join key that avoids `Vec<Value>` allocation for
/// single-column i64/string keys.
#[derive(Debug, Clone, Hash, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum JoinKeyValue {
    I32(i32),
    I64(i64),
    String(String),
    Multi(Vec<Value>),
}

impl JoinKeyValue {
    /// Whether this key is NULL (or all-NULL composite). NULL keys are pinned
    /// to Grace partition 0 so both sides agree on placement.
    pub fn is_nullish(&self) -> bool {
        match self {
            JoinKeyValue::Multi(values) => {
                !values.is_empty() && values.iter().all(|v| matches!(v, Value::Null(_)))
            }
            _ => false,
        }
    }
}

impl From<Value> for JoinKeyValue {
    fn from(value: Value) -> Self {
        match value {
            Value::Int(i) => JoinKeyValue::I32(i),
            Value::BigInt(i) => JoinKeyValue::I64(i),
            Value::String(s) => JoinKeyValue::String(s.to_string()),
            other => JoinKeyValue::Multi(vec![other]),
        }
    }
}

fn try_column_value(
    expr: &Expression,
    layout: &SlotLayout,
    columns: &[Vec<Value>],
    row_idx: usize,
) -> Option<Value> {
    let name = expr.as_variable()?;
    let slot = layout.slot_id(name)?;
    if slot < columns.len() {
        Some(columns[slot][row_idx].clone())
    } else {
        None
    }
}

fn eval_join_expr(expr: &Expression, ctx: &mut BorrowedRowContext) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, ctx)
        .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))
}

fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[Expression],
    columns: Option<(&[Vec<Value>], usize)>,
) -> Result<JoinKeyValue, QueryError> {
    if key_expressions.is_empty() {
        return Ok(JoinKeyValue::Multi(Vec::new()));
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut ctx = BorrowedRowContext::new(row, Arc::clone(&layout));

    let eval_one = |expr: &Expression, ctx: &mut BorrowedRowContext| -> Result<Value, QueryError> {
        if let Some((cols, row_idx)) = columns {
            if let Some(val) = try_column_value(expr, &layout, cols, row_idx) {
                return Ok(val);
            }
        }
        eval_join_expr(expr, ctx)
    };

    if key_expressions.len() == 1 {
        let value = eval_one(&key_expressions[0], &mut ctx)?;
        return Ok(JoinKeyValue::from(value));
    }

    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let value = eval_one(expr, &mut ctx)?;
        key.push(value);
    }
    Ok(JoinKeyValue::Multi(key))
}

/// Columnar build side for hash joins.
///
/// Build rows are accumulated column-major (one `Vec<Value>` per input
/// column) and the hash index maps each key to its row indices. Build costs
/// a single columnar copy per value — no per-row clones — and probe reads
/// rows back by index.
#[derive(Debug)]
pub struct HashJoinBuildSide {
    columns: Vec<Vec<Value>>,
    index: HashMap<JoinKeyValue, Vec<u32>>,
}

impl Default for HashJoinBuildSide {
    fn default() -> Self {
        Self::new()
    }
}

impl HashJoinBuildSide {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Append one input chunk: join keys are evaluated per visible row using
    /// the column fast path, chunk columns are moved into the column store,
    /// and each visible row is indexed by its key.
    ///
    /// This is the single build-side entry point: an attached selection is
    /// consumed in place (only visible column values move), so callers must
    /// not `materialize_selection` beforehand.
    pub fn insert_chunk(
        &mut self,
        chunk: &mut DataChunk,
        col_names: &[String],
        key_expressions: &[Expression],
    ) -> Result<(), QueryError> {
        chunk.materialize_columns();
        let cols = chunk.columns.as_deref().ok_or_else(|| {
            QueryError::execution("HashJoinBuildSide: empty chunk columns".to_string())
        })?;
        debug_assert_eq!(
            chunk.rows.len(),
            cols.first().map_or(0, Vec::len),
            "row/column count mismatch: chunk has rows without columnar data"
        );
        let visible = chunk.visible_indices();
        let base = self.row_count();
        for (pos, row_idx) in visible.iter().enumerate() {
            let row = &chunk.rows[*row_idx];
            let key = evaluate_join_key(row, col_names, key_expressions, Some((cols, *row_idx)))?;
            self.index.entry(key).or_default().push((base + pos) as u32);
        }
        let chunk_cols = chunk.columns.take().ok_or_else(|| {
            QueryError::execution("HashJoinBuildSide: empty chunk columns".to_string())
        })?;
        if self.columns.is_empty() {
            self.columns = if chunk.selection().is_none() {
                chunk_cols
            } else {
                chunk_cols
                    .into_iter()
                    .map(|col| visible.iter().map(|&i| col[i].clone()).collect())
                    .collect()
            };
        } else {
            if self.columns.len() != chunk_cols.len() {
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: chunk column count {} differs from build side column count {}",
                    chunk_cols.len(),
                    self.columns.len()
                )));
            }
            if chunk.selection().is_none() {
                for (target, src) in self.columns.iter_mut().zip(chunk_cols) {
                    target.extend(src);
                }
            } else {
                for (target, src) in self.columns.iter_mut().zip(chunk_cols) {
                    target.extend(visible.iter().map(|&i| src[i].clone()));
                }
            }
        }
        // The build chunk is fully consumed; drop rows/selection without a
        // second compaction pass.
        chunk.take_selection();
        chunk.rows.clear();
        chunk.typed_columns = None;
        Ok(())
    }

    /// Row indices matching a probe key.
    pub fn matching(&self, key: &JoinKeyValue) -> Option<&[u32]> {
        self.index.get(key).map(|v| v.as_slice())
    }

    /// Number of rows held in the columnar build store.
    pub fn row_count(&self) -> usize {
        self.columns.first().map_or(0, Vec::len)
    }

    /// Append the build row at `row_idx` directly into `target`.
    ///
    /// Avoids the intermediate `Vec` allocation of [`Self::row_at`] on the
    /// hot equi-join probe path: the caller pre-reserves capacity once and
    /// extends column values in place.
    pub fn append_row_to(&self, target: &mut Vec<Value>, row_idx: u32) {
        let idx = row_idx as usize;
        target.extend(self.columns.iter().map(|col| col[idx].clone()));
    }

    /// Insert one fully materialized row under a precomputed key.
    ///
    /// Used when rebuilding a Grace partition from spilled rows: keys come
    /// from re-evaluation, values move straight into the column store.
    pub fn insert_keyed_row(&mut self, key: JoinKeyValue, row: &[Value]) -> Result<(), QueryError> {
        let width = self.columns.len();
        if width == 0 {
            self.columns = row.iter().map(|v| vec![v.clone()]).collect();
        } else {
            if row.len() != width {
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: spilled row width {} differs from build width {}",
                    row.len(),
                    width,
                )));
            }
            for (target, value) in self.columns.iter_mut().zip(row.iter()) {
                target.push(value.clone());
            }
        }
        let idx = (self.row_count() - 1) as u32;
        self.index.entry(key).or_default().push(idx);
        Ok(())
    }

    /// Drain all indexed rows with their join keys, freeing build memory.
    ///
    /// Grace spill entry point: existing in-memory rows are re-partitioned to
    /// disk without re-evaluating key expressions.
    pub fn take_indexed_rows(&mut self) -> Vec<(JoinKeyValue, Vec<Value>)> {
        let index = std::mem::take(&mut self.index);
        let columns = std::mem::take(&mut self.columns);
        let row_count = columns.first().map_or(0, Vec::len);
        if row_count == 0 {
            return Vec::new();
        }
        let mut keys: Vec<Option<JoinKeyValue>> = (0..row_count).map(|_| None).collect();
        for (key, indices) in index {
            for idx in indices {
                let slot = keys
                    .get_mut(idx as usize)
                    .expect("build index out of range");
                debug_assert!(slot.is_none(), "duplicate build row index");
                *slot = Some(key.clone());
            }
        }
        let mut out = Vec::with_capacity(row_count);
        for (idx, key) in keys.into_iter().enumerate() {
            let key = key.expect("build row without a join key");
            let mut row = Vec::with_capacity(columns.len());
            for col in &columns {
                row.push(col[idx].clone());
            }
            out.push((key, row));
        }
        out
    }

    /// Materialize the row at the given index by cloning column values.
    pub fn row_at(&self, row_idx: u32) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.columns.len());
        self.append_row_to(&mut out, row_idx);
        out
    }

    pub fn clear(&mut self) {
        self.columns.clear();
        self.index.clear();
    }
}

#[derive(Debug)]
pub enum JoinOperatorKind {
    HashJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: HashJoinBuildSide,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        build_side_select: super::spec::BuildSide,
        grace: grace_join::GraceJoinState,
    },
    HashLeftJoin {
        join_condition: Option<Expression>,
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        build_side: HashJoinBuildSide,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        build_side_select: super::spec::BuildSide,
        grace: grace_join::GraceJoinState,
    },
    NestedLoopJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    InnerJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    LeftJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        build_done: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    RightJoin {
        join_condition: Option<Expression>,
        build_side_tuples: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    FullOuterJoin {
        join_condition: Option<Expression>,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        matched_right_indices: HashSet<usize>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
        phase: FullOuterJoinPhase,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
    CrossJoin {
        all_left_rows: Vec<Vec<Value>>,
        all_right_rows: Vec<Vec<Value>>,
        left_consumed: bool,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
        output_done: bool,
    },
    SemiJoin {
        join_condition: Option<Expression>,
        // NOT EXISTS semantics: keep left rows with NO matching right row.
        anti: bool,
        right_rows: Vec<Vec<Value>>,
        right_consumed: bool,
        memory_tracker: MemoryTracker,
        right_col_names: Vec<String>,
    },
}

/// Join operator.
///
/// Wraps [`JoinOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct JoinOperator {
    pub kind: JoinOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl JoinOperator {
    pub fn new(kind: JoinOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
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

    pub fn from_spec(
        spec: &super::spec::JoinSpec,
        memory_budget: &crate::executor::base::MemoryBudget,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::JoinSpec::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
            } => JoinOperatorKind::HashJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side: HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                build_side_select: *build_side,
                grace: grace_join::GraceJoinState::default(),
            },
            super::spec::JoinSpec::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
            } => JoinOperatorKind::HashLeftJoin {
                join_condition: join_condition.clone(),
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                build_side: HashJoinBuildSide::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                build_side_select: *build_side,
                grace: grace_join::GraceJoinState::default(),
            },
            super::spec::JoinSpec::NestedLoopJoin { join_condition } => {
                JoinOperatorKind::NestedLoopJoin {
                    join_condition: join_condition.clone(),
                    build_side_tuples: Vec::new(),
                    build_done: false,
                    memory_tracker: crate::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::InnerJoin { join_condition } => JoinOperatorKind::InnerJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::LeftJoin { join_condition } => JoinOperatorKind::LeftJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                build_done: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::RightJoin { join_condition } => JoinOperatorKind::RightJoin {
                join_condition: join_condition.clone(),
                build_side_tuples: Vec::new(),
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
            super::spec::JoinSpec::FullOuterJoin { join_condition } => {
                JoinOperatorKind::FullOuterJoin {
                    join_condition: join_condition.clone(),
                    left_rows: Vec::new(),
                    right_rows: Vec::new(),
                    matched_right_indices: std::collections::HashSet::new(),
                    result_iter: None,
                    phase: FullOuterJoinPhase::BuildingRight,
                    memory_tracker: crate::executor::base::MemoryTracker::new(
                        memory_budget.clone(),
                    ),
                    right_col_names: Vec::new(),
                }
            }
            super::spec::JoinSpec::CrossJoin => JoinOperatorKind::CrossJoin {
                all_left_rows: Vec::new(),
                all_right_rows: Vec::new(),
                left_consumed: false,
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
                output_done: false,
            },
            super::spec::JoinSpec::SemiJoin {
                join_condition,
                anti,
            } => JoinOperatorKind::SemiJoin {
                join_condition: join_condition.clone(),
                anti: *anti,
                right_rows: Vec::new(),
                right_consumed: false,
                memory_tracker: crate::executor::base::MemoryTracker::new(memory_budget.clone()),
                right_col_names: Vec::new(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match &self.kind {
            JoinOperatorKind::HashJoin { memory_tracker, .. }
            | JoinOperatorKind::HashLeftJoin { memory_tracker, .. }
            | JoinOperatorKind::NestedLoopJoin { memory_tracker, .. }
            | JoinOperatorKind::InnerJoin { memory_tracker, .. }
            | JoinOperatorKind::LeftJoin { memory_tracker, .. }
            | JoinOperatorKind::RightJoin { memory_tracker, .. }
            | JoinOperatorKind::FullOuterJoin { memory_tracker, .. }
            | JoinOperatorKind::CrossJoin { memory_tracker, .. }
            | JoinOperatorKind::SemiJoin { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        left.open()?;
        right.open()?;
        Ok(())
    }

    pub fn next(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        let runtime = &self.runtime;
        let output_layout = &self.output_layout;
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                build_side_select,
                grace,
            } => hash_join::next_hash_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                *build_side_select,
                grace,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::HashLeftJoin {
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                build_side_select,
                grace,
            } => hash_join::next_hash_left_join(
                join_condition,
                hash_keys,
                probe_keys,
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                *build_side_select,
                grace,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::NestedLoopJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => nested_loop_join::next_nested_loop_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::InnerJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => merge_join::next_inner_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::LeftJoin {
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
            } => merge_join::next_left_join(
                join_condition,
                build_side_tuples,
                build_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::RightJoin {
                join_condition,
                build_side_tuples,
                right_consumed,
                memory_tracker,
                right_col_names,
            } => merge_join::next_right_join(
                join_condition,
                build_side_tuples,
                right_consumed,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::FullOuterJoin {
                join_condition,
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                memory_tracker,
                right_col_names,
            } => merge_join::next_full_outer_join(
                join_condition,
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                memory_tracker,
                right_col_names,
                output_done,
            } => cross_semi_join::next_cross_join(
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                output_done,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
            JoinOperatorKind::SemiJoin {
                join_condition,
                anti,
                right_rows,
                right_consumed,
                memory_tracker,
                right_col_names,
            } => cross_semi_join::next_semi_join(
                join_condition,
                *anti,
                right_rows,
                right_consumed,
                memory_tracker,
                right_col_names,
                left,
                right,
                runtime,
                output_layout,
            ),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset per-run join state (hash tables, buffered sides, phase flags)
    /// and rewind both inputs so the join re-produces the same result set.
    pub fn reset(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<bool, QueryError> {
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                build_side,
                build_done,
                right_col_names,
                grace,
                memory_tracker,
                ..
            }
            | JoinOperatorKind::HashLeftJoin {
                build_side,
                build_done,
                right_col_names,
                grace,
                memory_tracker,
                ..
            } => {
                grace.cleanup_files();
                *grace = grace_join::GraceJoinState::default();
                *build_side = HashJoinBuildSide::new();
                *build_done = false;
                right_col_names.clear();
                memory_tracker.reset();
            }
            JoinOperatorKind::NestedLoopJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            }
            | JoinOperatorKind::InnerJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            }
            | JoinOperatorKind::LeftJoin {
                build_side_tuples,
                build_done,
                right_col_names,
                ..
            } => {
                build_side_tuples.clear();
                *build_done = false;
                right_col_names.clear();
            }
            JoinOperatorKind::RightJoin {
                build_side_tuples,
                right_consumed,
                right_col_names,
                ..
            } => {
                build_side_tuples.clear();
                *right_consumed = false;
                right_col_names.clear();
            }
            JoinOperatorKind::FullOuterJoin {
                left_rows,
                right_rows,
                matched_right_indices,
                result_iter,
                phase,
                right_col_names,
                ..
            } => {
                left_rows.clear();
                right_rows.clear();
                matched_right_indices.clear();
                *result_iter = None;
                *phase = FullOuterJoinPhase::BuildingRight;
                right_col_names.clear();
            }
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                left_consumed,
                right_consumed,
                right_col_names,
                output_done,
                ..
            } => {
                all_left_rows.clear();
                all_right_rows.clear();
                *left_consumed = false;
                *right_consumed = false;
                right_col_names.clear();
                *output_done = false;
            }
            JoinOperatorKind::SemiJoin {
                right_rows,
                right_consumed,
                right_col_names,
                ..
            } => {
                right_rows.clear();
                *right_consumed = false;
                right_col_names.clear();
            }
        }
        left.reset()?;
        right.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        match &mut self.kind {
            JoinOperatorKind::HashJoin {
                build_side,
                memory_tracker,
                grace,
                ..
            } => {
                grace.cleanup_files();
                hash_join::close(memory_tracker, build_side)
            }
            JoinOperatorKind::HashLeftJoin {
                build_side,
                memory_tracker,
                grace,
                ..
            } => {
                grace.cleanup_files();
                hash_join::close(memory_tracker, build_side)
            }
            JoinOperatorKind::NestedLoopJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::InnerJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::LeftJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::RightJoin {
                build_side_tuples,
                memory_tracker,
                ..
            } => nested_loop_join::close(memory_tracker, build_side_tuples),
            JoinOperatorKind::FullOuterJoin {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => merge_join::close_full_outer(memory_tracker, left_rows, right_rows),
            JoinOperatorKind::CrossJoin {
                all_left_rows,
                all_right_rows,
                memory_tracker,
                ..
            } => cross_semi_join::close_cross(memory_tracker, all_left_rows, all_right_rows),
            JoinOperatorKind::SemiJoin {
                right_rows,
                memory_tracker,
                ..
            } => cross_semi_join::close_semi(memory_tracker, right_rows),
        }
    }

    /// Spill the hash join build side to partitioned runs.
    ///
    /// Only effective during the build phase (before `build_done`): buffered
    /// build rows are re-partitioned to disk and the remaining build input is
    /// routed straight to the spill writers by the build loop. Once the probe
    /// phase has started the in-memory table must stay resident and this is a
    /// no-op; other join kinds keep their budget-failure semantics.
    pub fn spill_with_manager(
        &mut self,
        sm: &Arc<crate::executor::streaming::spill::SpillManager>,
    ) -> Result<(), graphdb_core::error::QueryError> {
        let (build_side, build_done, memory_tracker, right_col_names, grace) = match &mut self.kind
        {
            JoinOperatorKind::HashJoin {
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
                ..
            }
            | JoinOperatorKind::HashLeftJoin {
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
                ..
            } => (
                build_side,
                build_done,
                memory_tracker,
                right_col_names,
                grace,
            ),
            _ => return Ok(()),
        };
        if *build_done || grace.partitioned.is_some() {
            return Ok(());
        }
        if build_side.row_count() == 0 && grace.pending_build.is_none() {
            return Ok(());
        }
        grace_join::spill_build_side(
            Arc::clone(sm),
            build_side,
            memory_tracker,
            right_col_names,
            grace,
        )
    }

    pub fn spilled_bytes(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spilled_bytes,
            _ => 0,
        }
    }

    pub fn spill_count(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spill_runs,
            _ => 0,
        }
    }

    pub fn spilled_rows(&self) -> u64 {
        match &self.kind {
            JoinOperatorKind::HashJoin { grace, .. }
            | JoinOperatorKind::HashLeftJoin { grace, .. } => grace.spilled_rows,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_from_columns(cols: Vec<Vec<Value>>) -> DataChunk {
        let names: Vec<String> = (0..cols.len()).map(|i| format!("c{i}")).collect();
        DataChunk::from_columns(cols, Arc::new(SlotLayout::from_names(&names)))
    }

    #[test]
    fn insert_chunk_accumulates_across_chunks() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::string("a"), Value::string("b")],
        ]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![vec![Value::Int(3)], vec![Value::string("c")]]);
        side.insert_chunk(&mut c2, &[], &[]).unwrap();
        assert_eq!(side.columns.len(), 2);
        assert_eq!(
            side.columns[0],
            vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        );
        assert_eq!(side.row_at(2), vec![Value::Int(3), Value::string("c")]);
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 3);
    }

    #[test]
    fn insert_chunk_column_count_mismatch_is_error() {
        let mut side = HashJoinBuildSide::new();
        let mut c1 = chunk_from_columns(vec![vec![Value::Int(1)], vec![Value::Int(2)]]);
        side.insert_chunk(&mut c1, &[], &[]).unwrap();
        let mut c2 = chunk_from_columns(vec![
            vec![Value::Int(3)],
            vec![Value::Int(4)],
            vec![Value::Int(5)],
        ]);
        let err = side.insert_chunk(&mut c2, &[], &[]).unwrap_err();
        assert!(err.to_string().contains("column count"));
        assert_eq!(side.columns.len(), 2);
        assert_eq!(side.columns[0], vec![Value::Int(1)]);
    }

    #[test]
    fn insert_chunk_consumes_selection_in_place() {
        // Single entry point: an attached selection is consumed without a
        // prior materialization, and only visible rows land in the store.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = chunk_from_columns(vec![
            vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            vec![Value::string("a"), Value::string("b"), Value::string("c")],
        ])
        .with_selection(vec![0, 2]);
        side.insert_chunk(&mut chunk, &[], &[]).unwrap();
        assert_eq!(side.row_count(), 2);
        assert_eq!(side.columns[0], vec![Value::Int(1), Value::Int(3)]);
        assert!(chunk.selection().is_none());
        assert!(chunk.rows.is_empty());
        let indexed_rows: usize = side.index.values().map(|v| v.len()).sum();
        assert_eq!(indexed_rows, 2);
    }

    #[test]
    #[should_panic(expected = "row/column count mismatch")]
    fn insert_chunk_rejects_schema_less_chunk() {
        // Rows carrying values that no schema column can address would be
        // silently dropped from the build side; the invariant guard must fire.
        let mut side = HashJoinBuildSide::new();
        let mut chunk = DataChunk::new_with_layout(
            vec![vec![Value::Int(1)]],
            Arc::new(SlotLayout::from_names(&[])),
        );
        let _ = side.insert_chunk(&mut chunk, &[], &[]);
    }
}

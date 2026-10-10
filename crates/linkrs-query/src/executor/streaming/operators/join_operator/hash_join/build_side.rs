/// Specialized hash join key that avoids `Vec<Value>` allocation for
/// single-column i64/string keys.
use std::collections::HashMap;
use std::sync::Arc;

use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::{gather_typed_column, DataChunk};
use crate::executor::streaming::context::BorrowedRowContext;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::error::QueryError;
use linkrs_core::types::expr::Expression;
use linkrs_core::Value;

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

fn eval_join_expr(expr: &Expression, ctx: &mut BorrowedRowContext) -> Result<Value, QueryError> {
    ExpressionEvaluator::evaluate(expr, ctx)
        .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {}", e)))
}

pub(in crate::executor::streaming::operators::join_operator) fn evaluate_join_key(
    row: &[Value],
    col_names: &[String],
    key_expressions: &[Expression],
) -> Result<JoinKeyValue, QueryError> {
    if key_expressions.is_empty() {
        return Ok(JoinKeyValue::Multi(Vec::new()));
    }

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut ctx = BorrowedRowContext::new(row, Arc::clone(&layout));

    if key_expressions.len() == 1 {
        let value = eval_join_expr(&key_expressions[0], &mut ctx)?;
        return Ok(JoinKeyValue::from(value));
    }

    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let value = eval_join_expr(expr, &mut ctx)?;
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
    /// Build-side column store (authoritative row-indexed storage).
    /// Unrelated to the removed `DataChunk.columns` lazy shim.
    pub(crate) build_columns: Vec<Vec<Value>>,
    pub(crate) index: HashMap<JoinKeyValue, Vec<u32>>,
}

impl Default for HashJoinBuildSide {
    fn default() -> Self {
        Self::new()
    }
}

impl HashJoinBuildSide {
    pub fn new() -> Self {
        Self {
            build_columns: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Append one input chunk: join keys are evaluated per visible row,
    /// visible rows move into the column store, and each visible row is
    /// indexed by its key.
    ///
    /// This is the single build-side entry point: an attached selection is
    /// consumed in place (only visible rows move), so callers must
    /// not `materialize_selection` beforehand. Values move from the typed
    /// column layout when present (gathered per visible row) and fall back
    /// to row-major clones otherwise; keys evaluate per row through the
    /// scalar interpreter.
    pub fn insert_chunk(
        &mut self,
        chunk: &mut DataChunk,
        col_names: &[String],
        key_expressions: &[Expression],
    ) -> Result<(), QueryError> {
        let visible = chunk.visible_indices();
        let num_cols = chunk.num_columns();
        debug_assert!(
            chunk.rows.iter().all(|row| row.len() == num_cols),
            "row/column count mismatch: chunk has rows without columnar data"
        );
        let base = self.row_count();
        for (pos, row_idx) in visible.iter().enumerate() {
            let row = &chunk.rows[*row_idx];
            let key = evaluate_join_key(row, col_names, key_expressions)?;
            self.index.entry(key).or_default().push((base + pos) as u32);
        }
        // Prefer the typed layout: gather visible rows per column in one pass
        // instead of cloning value-by-value out of `rows`. The typed layout is
        // consumed here, so dropping it afterwards is not a wasted build.
        let typed = chunk.typed_columns.take();
        let typed_usable = typed.as_ref().is_some_and(|cols| cols.len() == num_cols);
        if typed_usable {
            if !self.build_columns.is_empty() && num_cols != self.build_columns.len() {
                chunk.typed_columns = typed;
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: chunk column count {} differs from build side column count {}",
                    num_cols,
                    self.build_columns.len()
                )));
            }
            let cols = typed.as_ref().expect("typed layout checked usable");
            if self.build_columns.is_empty() {
                // First chunk defines the build width; only visible rows move.
                self.build_columns = cols
                    .iter()
                    .map(|col| gather_typed_column(col, &visible).to_values())
                    .collect();
            } else {
                for (target, col) in self.build_columns.iter_mut().zip(cols.iter()) {
                    target.extend(gather_typed_column(col, &visible).to_values());
                }
            }
        } else {
            if self.build_columns.is_empty() {
                // First chunk defines the build width; only visible rows move.
                self.build_columns = (0..num_cols)
                    .map(|j| visible.iter().map(|&i| chunk.rows[i][j].clone()).collect())
                    .collect();
            } else {
                if num_cols != self.build_columns.len() {
                    chunk.typed_columns = typed;
                    return Err(QueryError::execution(format!(
                        "HashJoinBuildSide: chunk column count {} differs from build side column count {}",
                        num_cols,
                        self.build_columns.len()
                    )));
                }
                for (j, target) in self.build_columns.iter_mut().enumerate() {
                    target.extend(visible.iter().map(|&i| chunk.rows[i][j].clone()));
                }
            }
        }
        // The build chunk is fully consumed; drop rows/selection without a
        // second compaction pass. A typed layout dropped without being
        // consumed counts as a wasted build so a high build cost with no
        // reader becomes visible.
        chunk.take_selection();
        chunk.rows.clear();
        if !typed_usable && typed.is_some() {
            if let Some(stats) = &chunk.columnar_stats {
                stats.record_wasted_build();
            }
        }
        Ok(())
    }

    /// Row indices matching a probe key.
    pub fn matching(&self, key: &JoinKeyValue) -> Option<&[u32]> {
        self.index.get(key).map(|v| v.as_slice())
    }

    /// Number of rows held in the columnar build store.
    pub fn row_count(&self) -> usize {
        self.build_columns.first().map_or(0, Vec::len)
    }

    /// Append the build row at `row_idx` directly into `target`.
    ///
    /// Avoids the intermediate `Vec` allocation of [`Self::row_at`] on the
    /// hot equi-join probe path: the caller pre-reserves capacity once and
    /// extends column values in place.
    pub fn append_row_to(&self, target: &mut Vec<Value>, row_idx: u32) {
        let idx = row_idx as usize;
        target.extend(self.build_columns.iter().map(|col| col[idx].clone()));
    }

    /// Insert one fully materialized row under a precomputed key.
    ///
    /// Used when rebuilding a Grace partition from spilled rows: keys come
    /// from re-evaluation, values move straight into the column store.
    pub fn insert_keyed_row(&mut self, key: JoinKeyValue, row: &[Value]) -> Result<(), QueryError> {
        let width = self.build_columns.len();
        if width == 0 {
            self.build_columns = row.iter().map(|v| vec![v.clone()]).collect();
        } else {
            if row.len() != width {
                return Err(QueryError::execution(format!(
                    "HashJoinBuildSide: spilled row width {} differs from build width {}",
                    row.len(),
                    width,
                )));
            }
            for (target, value) in self.build_columns.iter_mut().zip(row.iter()) {
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
        let columns = std::mem::take(&mut self.build_columns);
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
        let mut out = Vec::with_capacity(self.build_columns.len());
        self.append_row_to(&mut out, row_idx);
        out
    }

    pub fn clear(&mut self) {
        self.build_columns.clear();
        self.index.clear();
    }
}

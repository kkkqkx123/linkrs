use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::executor::base::{MemoryBudget, MemoryTracker};
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;

#[derive(Debug)]
pub enum SetOperatorKind {
    Union {
        seen_rows: HashSet<String>,
        left_consumed: bool,
        memory_tracker: MemoryTracker,
    },
    UnionAll {
        left_consumed: bool,
    },
    Intersect {
        left_rows: Vec<Vec<Value>>,
        right_rows: HashSet<String>,
        left_buffered: bool,
        right_buffered: bool,
        emitted: bool,
        memory_tracker: MemoryTracker,
    },
    Except {
        exclude_rows: HashSet<String>,
        right_buffered: bool,
        memory_tracker: MemoryTracker,
    },
    Minus {
        exclude_rows: HashSet<String>,
        right_buffered: bool,
        memory_tracker: MemoryTracker,
    },
}

/// Set operator.
///
/// Wraps [`SetOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct SetOperator {
    pub kind: SetOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl SetOperator {
    pub fn from_spec(
        spec: &super::spec::SetSpec,
        budget: &MemoryBudget,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::SetSpec::Union => SetOperatorKind::Union {
                seen_rows: std::collections::HashSet::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::UnionAll => SetOperatorKind::UnionAll {
                left_consumed: false,
            },
            super::spec::SetSpec::Intersect => SetOperatorKind::Intersect {
                left_rows: Vec::new(),
                right_rows: std::collections::HashSet::new(),
                left_buffered: false,
                right_buffered: false,
                emitted: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::Except => SetOperatorKind::Except {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::Minus => SetOperatorKind::Minus {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: SetOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
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

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match &self.kind {
            SetOperatorKind::Union { memory_tracker, .. }
            | SetOperatorKind::Intersect { memory_tracker, .. }
            | SetOperatorKind::Except { memory_tracker, .. }
            | SetOperatorKind::Minus { memory_tracker, .. } => memory_tracker,
            SetOperatorKind::UnionAll { .. } => {
                panic!("memory_tracker called on variant without memory tracking")
            }
        }
    }

    pub fn open(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match &mut self.kind {
            SetOperatorKind::Union { .. }
            | SetOperatorKind::UnionAll { .. }
            | SetOperatorKind::Intersect { .. }
            | SetOperatorKind::Except { .. }
            | SetOperatorKind::Minus { .. } => {
                left.open()?;
                right.open()?;
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
            SetOperatorKind::Union {
                seen_rows,
                left_consumed,
                memory_tracker,
                ..
            } => loop {
                if let Some(rt) = self.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if !*left_consumed {
                    if let Some(mut chunk) = left.advance()? {
                        chunk.materialize_selection_by("Set");
                        let mut result_rows = Vec::new();
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            if !seen_rows.contains(&row_str) {
                                memory_tracker.try_reserve(row_str.len())?;
                                seen_rows.insert(row_str);
                                result_rows.push(row);
                            }
                        }
                        if !result_rows.is_empty() {
                            return Ok(Some(DataChunk::new_with_layout(
                                result_rows,
                                Arc::clone(&self.output_layout),
                            )));
                        }
                        continue;
                    } else {
                        *left_consumed = true;
                    }
                }

                if let Some(mut chunk) = right.advance()? {
                    chunk.materialize_selection_by("Set");
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        if !seen_rows.contains(&row_str) {
                            memory_tracker.try_reserve(row_str.len())?;
                            seen_rows.insert(row_str);
                            result_rows.push(row);
                        }
                    }
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            result_rows,
                            Arc::clone(&self.output_layout),
                        )));
                    }
                    continue;
                }
                return Ok(None);
            },

            SetOperatorKind::UnionAll { left_consumed, .. } => loop {
                if !*left_consumed {
                    if let Some(mut chunk) = left.advance()? {
                        chunk.materialize_selection_by("Set");
                        if !chunk.is_empty() {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk.rows,
                                Arc::clone(&self.output_layout),
                            )));
                        }
                    } else {
                        *left_consumed = true;
                    }
                }

                if let Some(mut chunk) = right.advance()? {
                    chunk.materialize_selection_by("Set");
                    if !chunk.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            chunk.rows,
                            Arc::clone(&self.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },

            SetOperatorKind::Intersect {
                left_rows,
                right_rows,
                left_buffered,
                right_buffered,
                emitted,
                memory_tracker,
                ..
            } => {
                if *emitted {
                    return Ok(None);
                }
                if !*left_buffered {
                    while let Some(mut chunk) = left.advance()? {
                        chunk.materialize_selection_by("Set");
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        left_rows.extend(chunk.rows);
                    }
                    *left_buffered = true;
                }

                if !*right_buffered {
                    while let Some(mut chunk) = right.advance()? {
                        chunk.materialize_selection_by("Set");
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            memory_tracker.try_reserve(row_str.len())?;
                            right_rows.insert(row_str);
                        }
                    }
                    *right_buffered = true;
                }

                let result_rows: Vec<Vec<Value>> = left_rows
                    .iter()
                    .filter(|row| right_rows.contains(&format!("{:?}", row)))
                    .cloned()
                    .collect();

                if result_rows.is_empty() {
                    Ok(None)
                } else {
                    *emitted = true;
                    Ok(Some(DataChunk::new_with_layout(
                        result_rows,
                        Arc::clone(&self.output_layout),
                    )))
                }
            }

            SetOperatorKind::Except {
                exclude_rows,
                right_buffered,
                memory_tracker,
                ..
            } => loop {
                if let Some(rt) = self.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                if !*right_buffered {
                    while let Some(mut chunk) = right.advance()? {
                        chunk.materialize_selection_by("Set");
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            memory_tracker.try_reserve(row_str.len())?;
                            exclude_rows.insert(row_str);
                        }
                    }
                    *right_buffered = true;
                }

                if let Some(mut chunk) = left.advance()? {
                    chunk.materialize_selection_by("Set");
                    let result_rows: Vec<Vec<Value>> = chunk
                        .rows
                        .into_iter()
                        .filter(|row| !exclude_rows.contains(&format!("{:?}", row)))
                        .collect();

                    if result_rows.is_empty() {
                        continue;
                    }
                    return Ok(Some(DataChunk::new_with_layout(
                        result_rows,
                        Arc::clone(&self.output_layout),
                    )));
                }
                return Ok(None);
            },

            SetOperatorKind::Minus {
                exclude_rows,
                right_buffered,
                memory_tracker,
                ..
            } => {
                if !*right_buffered {
                    while let Some(mut chunk) = right.advance()? {
                        chunk.materialize_selection_by("Set");
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            memory_tracker.try_reserve(row_str.len())?;
                            exclude_rows.insert(row_str);
                        }
                    }
                    *right_buffered = true;
                }

                loop {
                    if let Some(rt) = self.runtime.as_ref() {
                        rt.ensure_not_cancelled()?;
                    }
                    if let Some(mut chunk) = left.advance()? {
                        chunk.materialize_selection_by("Set");
                        let mut result_rows = Vec::new();
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            if !exclude_rows.contains(&row_str) {
                                result_rows.push(row);
                            }
                        }
                        if !result_rows.is_empty() {
                            return Ok(Some(DataChunk::new_with_layout(
                                result_rows,
                                Arc::clone(&self.output_layout),
                            )));
                        }
                        continue;
                    }
                    return Ok(None);
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset per-run set state (seen/exclude sets, buffered sides, phase
    /// flags) and rewind both inputs so the set operation re-runs cleanly.
    pub fn reset(
        &mut self,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<bool, QueryError> {
        match &mut self.kind {
            SetOperatorKind::Union {
                seen_rows,
                left_consumed,
                ..
            } => {
                seen_rows.clear();
                *left_consumed = false;
            }
            SetOperatorKind::UnionAll { left_consumed } => *left_consumed = false,
            SetOperatorKind::Intersect {
                left_rows,
                right_rows,
                left_buffered,
                right_buffered,
                emitted,
                ..
            } => {
                left_rows.clear();
                right_rows.clear();
                *left_buffered = false;
                *right_buffered = false;
                *emitted = false;
            }
            SetOperatorKind::Except {
                exclude_rows,
                right_buffered,
                ..
            }
            | SetOperatorKind::Minus {
                exclude_rows,
                right_buffered,
                ..
            } => {
                exclude_rows.clear();
                *right_buffered = false;
            }
        }
        left.reset()?;
        right.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        match &mut self.kind {
            SetOperatorKind::Union {
                seen_rows,
                memory_tracker,
                ..
            } => {
                memory_tracker.reset();
                seen_rows.clear();
            }
            SetOperatorKind::UnionAll { .. } => {}
            SetOperatorKind::Intersect {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => {
                memory_tracker.reset();
                left_rows.clear();
                right_rows.clear();
            }
            SetOperatorKind::Except {
                exclude_rows,
                memory_tracker,
                ..
            }
            | SetOperatorKind::Minus {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                memory_tracker.reset();
                exclude_rows.clear();
            }
        }
        Ok(())
    }

    pub fn spill_with_manager(
        &mut self,
        sm: &crate::executor::streaming::spill::SpillManager,
    ) -> Result<(), crate::core::error::QueryError> {
        match &mut self.kind {
            SetOperatorKind::Union {
                seen_rows,
                memory_tracker,
                ..
            } => {
                if !seen_rows.is_empty() {
                    let rows: Vec<Vec<crate::core::Value>> = seen_rows
                        .iter()
                        .map(|s| vec![crate::core::Value::string(s.clone())])
                        .collect();
                    let mut writer = sm.create_writer()?;
                    writer.write_rows(&rows)?;
                    seen_rows.clear();
                    memory_tracker.reset();
                }
                Ok(())
            }
            SetOperatorKind::Intersect {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => {
                if !left_rows.is_empty() {
                    let mut writer = sm.create_writer()?;
                    writer.write_rows(left_rows)?;
                    left_rows.clear();
                    memory_tracker.reset();
                }
                if !right_rows.is_empty() {
                    right_rows.clear();
                }
                Ok(())
            }
            SetOperatorKind::Except {
                exclude_rows,
                memory_tracker,
                ..
            }
            | SetOperatorKind::Minus {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                if !exclude_rows.is_empty() {
                    exclude_rows.clear();
                    memory_tracker.reset();
                }
                Ok(())
            }
            SetOperatorKind::UnionAll { .. } => Ok(()),
        }
    }

    pub fn spilled_bytes(&self) -> u64 {
        0
    }
}

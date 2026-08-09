use std::collections::HashSet;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;

#[derive(Debug)]
pub enum SetOperator {
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

impl SetOperator {
    pub fn from_spec(spec: &super::spec::SetSpec, budget: &MemoryBudget) -> Self {
        match spec {
            super::spec::SetSpec::Union => Self::Union {
                seen_rows: std::collections::HashSet::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::UnionAll => Self::UnionAll {
                left_consumed: false,
            },
            super::spec::SetSpec::Intersect => Self::Intersect {
                left_rows: Vec::new(),
                right_rows: std::collections::HashSet::new(),
                left_buffered: false,
                right_buffered: false,
                emitted: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::Except => Self::Except {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::spec::SetSpec::Minus => Self::Minus {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::Union { memory_tracker, .. }
            | Self::Intersect { memory_tracker, .. }
            | Self::Except { memory_tracker, .. }
            | Self::Minus { memory_tracker, .. } => memory_tracker,
            Self::UnionAll { .. } => {
                panic!("memory_tracker called on variant without memory tracking")
            }
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Union { .. }
            | Self::UnionAll { .. }
            | Self::Intersect { .. }
            | Self::Except { .. } => {
                left.open()?;
                right.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
            Self::Minus { .. } => {
                left.open()?;
                base.lifecycle.mark_opened();
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Union {
                seen_rows,
                left_consumed,
                memory_tracker,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
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
                                Arc::clone(&base.output_layout),
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                    continue;
                }
                return Ok(None);
            },

            Self::UnionAll { left_consumed, .. } => loop {
                if !*left_consumed {
                    if let Some(mut chunk) = left.advance()? {
                        chunk.materialize_selection_by("Set");
                        if !chunk.is_empty() {
                            return Ok(Some(DataChunk::new_with_layout(
                                chunk.rows,
                                Arc::clone(&base.output_layout),
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
                            Arc::clone(&base.output_layout),
                        )));
                    }
                } else {
                    return Ok(None);
                }
            },

            Self::Intersect {
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
                        base.ensure_not_cancelled()?;
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
                        base.ensure_not_cancelled()?;
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
                        Arc::clone(&base.output_layout),
                    )))
                }
            }

            Self::Except {
                exclude_rows,
                right_buffered,
                memory_tracker,
                ..
            } => loop {
                base.ensure_not_cancelled()?;
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
                        Arc::clone(&base.output_layout),
                    )));
                }
                return Ok(None);
            },

            Self::Minus {
                exclude_rows,
                right_buffered,
                memory_tracker,
                ..
            } => {
                if !*right_buffered {
                    right.open()?;
                    while let Some(mut chunk) = right.advance()? {
                        chunk.materialize_selection_by("Set");
                        base.ensure_not_cancelled()?;
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            memory_tracker.try_reserve(row_str.len())?;
                            exclude_rows.insert(row_str);
                        }
                    }
                    right.close()?;
                    *right_buffered = true;
                }

                loop {
                    base.ensure_not_cancelled()?;
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
                                Arc::clone(&base.output_layout),
                            )));
                        }
                        continue;
                    }
                    return Ok(None);
                }
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        base.lifecycle.mark_stopped();
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Union {
                seen_rows,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    seen_rows.clear();
                    base.lifecycle.mark_closed();
                    Ok(())
                } else {
                    Ok(())
                }
            }
            Self::UnionAll { .. } => {
                if base.lifecycle.can_close() {
                    base.lifecycle.mark_closed();
                    Ok(())
                } else {
                    Ok(())
                }
            }
            Self::Intersect {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    left_rows.clear();
                    right_rows.clear();
                    base.lifecycle.mark_closed();
                    Ok(())
                } else {
                    Ok(())
                }
            }
            Self::Except {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                if base.lifecycle.can_close() {
                    memory_tracker.reset();
                    exclude_rows.clear();
                    base.lifecycle.mark_closed();
                    Ok(())
                } else {
                    Ok(())
                }
            }
            Self::Minus {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                memory_tracker.reset();
                exclude_rows.clear();
                base.lifecycle.mark_closed();
                Ok(())
            }
        }
    }

    pub fn spill_with_manager(
        &mut self,
        sm: &crate::query::executor::streaming::spill::SpillManager,
    ) -> Result<(), crate::core::error::QueryError> {
        match self {
            Self::Union {
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
            Self::Intersect {
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
            Self::Except {
                exclude_rows,
                memory_tracker,
                ..
            }
            | Self::Minus {
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
            Self::UnionAll { .. } => Ok(()),
        }
    }

    pub fn spilled_bytes(&self) -> u64 {
        0
    }
}

use std::collections::HashSet;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operator_base::OperatorBase;

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
    pub fn from_spec(spec: &super::super::operator_spec::SetSpec, budget: &MemoryBudget) -> Self {
        match spec {
            super::super::operator_spec::SetSpec::Union => Self::Union {
                seen_rows: std::collections::HashSet::new(),
                left_consumed: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::super::operator_spec::SetSpec::UnionAll => Self::UnionAll {
                left_consumed: false,
            },
            super::super::operator_spec::SetSpec::Intersect => Self::Intersect {
                left_rows: Vec::new(),
                right_rows: std::collections::HashSet::new(),
                left_buffered: false,
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::super::operator_spec::SetSpec::Except => Self::Except {
                exclude_rows: std::collections::HashSet::new(),
                right_buffered: false,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            super::super::operator_spec::SetSpec::Minus => Self::Minus {
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
                    if let Some(chunk) = left.advance()? {
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
                            return Ok(Some(DataChunk::from_rows(result_rows)));
                        }
                        continue;
                    } else {
                        *left_consumed = true;
                    }
                }

                if let Some(chunk) = right.advance()? {
                    let mut result_rows = Vec::new();
                    for row in chunk.rows {
                        let row_str = format!("{:?}", row);
                        if !seen_rows.contains(&row_str) {
                            seen_rows.insert(row_str);
                            result_rows.push(row);
                        }
                    }
                    if !result_rows.is_empty() {
                        return Ok(Some(DataChunk::from_rows(result_rows)));
                    }
                    continue;
                }
                return Ok(None);
            },

            Self::UnionAll { left_consumed, .. } => {
                if !*left_consumed {
                    if let Some(chunk) = left.advance()? {
                        return Ok(Some(chunk));
                    } else {
                        *left_consumed = true;
                    }
                }

                right.advance()
            }

            Self::Intersect {
                left_rows,
                right_rows,
                left_buffered,
                right_buffered,
                memory_tracker,
                ..
            } => {
                if !*left_buffered {
                    while let Some(chunk) = left.advance()? {
                        base.ensure_not_cancelled()?;
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        left_rows.extend(chunk.rows);
                    }
                    *left_buffered = true;
                }

                if !*right_buffered {
                    while let Some(chunk) = right.advance()? {
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
                    Ok(Some(DataChunk::from_rows(result_rows)))
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
                    while let Some(chunk) = right.advance()? {
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            memory_tracker.try_reserve(row_str.len())?;
                            exclude_rows.insert(row_str);
                        }
                    }
                    *right_buffered = true;
                }

                if let Some(chunk) = left.advance()? {
                    let result_rows: Vec<Vec<Value>> = chunk
                        .rows
                        .into_iter()
                        .filter(|row| !exclude_rows.contains(&format!("{:?}", row)))
                        .collect();

                    if result_rows.is_empty() {
                        continue;
                    }
                    return Ok(Some(DataChunk::from_rows(result_rows)));
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
                    while let Some(chunk) = right.advance()? {
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
                    if let Some(chunk) = left.advance()? {
                        let mut result_rows = Vec::new();
                        for row in chunk.rows {
                            let row_str = format!("{:?}", row);
                            if !exclude_rows.contains(&row_str) {
                                result_rows.push(row);
                            }
                        }
                        if !result_rows.is_empty() {
                            return Ok(Some(DataChunk::from_rows(result_rows)));
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
        _base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Union { .. }
            | Self::UnionAll { .. }
            | Self::Intersect { .. }
            | Self::Except { .. } => {
                left.stop()?;
                right.stop()
            }
            Self::Minus { .. } => {
                left.stop()?;
                Ok(())
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
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
                    let left_err = left.close().err();
                    let right_err = right.close().err();
                    base.lifecycle.mark_closed();
                    match (left_err, right_err) {
                        (Some(e), _) => Err(e),
                        (_, Some(e)) => Err(e),
                        _ => Ok(()),
                    }
                } else {
                    Ok(())
                }
            }
            Self::UnionAll { .. } => {
                if base.lifecycle.can_close() {
                    let left_err = left.close().err();
                    let right_err = right.close().err();
                    base.lifecycle.mark_closed();
                    match (left_err, right_err) {
                        (Some(e), _) => Err(e),
                        (_, Some(e)) => Err(e),
                        _ => Ok(()),
                    }
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
                    let left_err = left.close().err();
                    let right_err = right.close().err();
                    base.lifecycle.mark_closed();
                    match (left_err, right_err) {
                        (Some(e), _) => Err(e),
                        (_, Some(e)) => Err(e),
                        _ => Ok(()),
                    }
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
                    let left_err = left.close().err();
                    let right_err = right.close().err();
                    base.lifecycle.mark_closed();
                    match (left_err, right_err) {
                        (Some(e), _) => Err(e),
                        (_, Some(e)) => Err(e),
                        _ => Ok(()),
                    }
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
                let left_err = left.close().err();
                base.lifecycle.mark_closed();
                match left_err {
                    Some(e) => Err(e),
                    None => Ok(()),
                }
            }
        }
    }
}

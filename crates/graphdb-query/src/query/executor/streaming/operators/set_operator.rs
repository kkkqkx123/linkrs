use std::collections::HashSet;

use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::base::MemoryTracker;
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
                base.opened = true;
                Ok(())
            }
            Self::Minus { .. } => {
                left.open()?;
                base.opened = true;
                Ok(())
            }
        }
    }

    pub fn next(
        &mut self,
        _base: &mut OperatorBase,
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
                        for row in &chunk.rows {
                            memory_tracker.try_reserve_row(row)?;
                        }
                        left_rows.extend(chunk.rows);
                    }
                    *left_buffered = true;
                }

                if !*right_buffered {
                    while let Some(chunk) = right.advance()? {
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
                if base.opened {
                    let mem = seen_rows.len() * 256;
                    memory_tracker.release(mem);
                    seen_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::UnionAll { .. } => {
                if base.opened {
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::Intersect {
                left_rows,
                right_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem_rows = MemoryBudget::estimate_rows_memory(left_rows);
                    let mem_set = right_rows.len() * 256;
                    memory_tracker.release(mem_rows + mem_set);
                    left_rows.clear();
                    right_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::Except {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                if base.opened {
                    let mem = exclude_rows.len() * 256;
                    memory_tracker.release(mem);
                    exclude_rows.clear();
                    left.close()?;
                    right.close()?;
                    base.opened = false;
                }
                Ok(())
            }
            Self::Minus {
                exclude_rows,
                memory_tracker,
                ..
            } => {
                let mem = exclude_rows.len() * 256;
                memory_tracker.release(mem);
                exclude_rows.clear();
                left.close()?;
                Ok(())
            }
        }
    }
}

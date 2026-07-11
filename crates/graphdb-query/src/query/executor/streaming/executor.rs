//! StreamingExecutor: Thin dispatch layer over domain-specific operator enums

use std::sync::Arc;
use std::time::Instant;

use super::chunk::DataChunk;
use super::runtime::ExecutionRuntime;
use crate::core::error::QueryError;
use crate::query::executor::base::{MemoryTracker, Spillable};

pub use super::context::ValueRowContext;
pub use super::helpers::{aggregation, comparison, conversion};
pub use super::operator_base::OperatorBase;

use super::operators::apply_operator::ApplyOperator;
use super::operators::blocking_operator::BlockingOperator;
use super::operators::ddl_operator::DdlOperator;
use super::operators::fulltext_operator::FulltextOperator;
use super::operators::graph_operator::GraphOperator;
use super::operators::join_operator::JoinOperator;
use super::operators::set_operator::SetOperator;
use super::operators::sink_operator::SinkOperator;
use super::operators::source_operator::SourceOperator;
use super::operators::txn_operator::TxnOperator;
use super::operators::unary_operator::UnaryOperator;
use super::operators::vector_operator::VectorOperator;


/// Sort direction for ORDER BY clause
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Phase for FullOuterJoin execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullOuterJoinPhase {
    BuildingRight,
    ProbeLeft,
    EmitUnmatchedRight,
}

/// StreamingExecutor: 10-variant dispatch enum over domain-specific operators.
///
/// Each variant holds an OperatorBase (shared fields), zero or more child
/// executors (Box<StreamingExecutor>), and a domain-specific operator enum
/// that implements the per-operator lifecycle logic.
#[derive(Debug)]
pub enum StreamingExecutor {
    Source(OperatorBase, SourceOperator),
    Unary(OperatorBase, Box<StreamingExecutor>, UnaryOperator),
    Join(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        JoinOperator,
    ),
    Set(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        SetOperator,
    ),
    Apply(
        OperatorBase,
        Box<StreamingExecutor>,
        Box<StreamingExecutor>,
        ApplyOperator,
    ),
    Blocking(OperatorBase, Box<StreamingExecutor>, BlockingOperator),
    Graph(OperatorBase, Box<StreamingExecutor>, GraphOperator),
    Sink(OperatorBase, Box<StreamingExecutor>, SinkOperator),
    Ddl(OperatorBase, Box<StreamingExecutor>, DdlOperator),
    Fulltext(OperatorBase, Box<StreamingExecutor>, FulltextOperator),
    Vector(OperatorBase, Box<StreamingExecutor>, VectorOperator),
    Txn(OperatorBase, Box<StreamingExecutor>, TxnOperator),
}

impl StreamingExecutor {
    /// Recursively set the runtime on this operator and all children.
    pub fn set_runtime(&mut self, rt: Option<Arc<ExecutionRuntime>>) {
        self.base_mut().runtime = rt.clone();
        for child in self.children_mut() {
            child.set_runtime(rt.clone());
        }
    }

    /// Return the plan node ID of this operator.
    pub fn plan_node_id(&self) -> i64 {
        self.base().plan_node_id
    }

    /// Access the runtime reference, if attached.
    pub fn get_runtime(&self) -> Option<&ExecutionRuntime> {
        self.base().runtime.as_deref()
    }

    /// Check cancellation via the attached runtime.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        self.base().ensure_not_cancelled()
    }

    /// Record profile timing for this operator.
    pub fn record_profile_timing(&self, phase: &str, elapsed_us: u64) {
        self.base().record_profile_timing(phase, elapsed_us);
    }

    /// Get peak memory from the memory_tracker, if this operator has one.
    pub fn peak_memory_bytes(&self) -> u64 {
        self.memory_tracker().map_or(0, |mt| mt.peak() as u64)
    }

    /// Record output row count in profile for this operator.
    pub fn record_profile_rows(&self, count: u64) {
        self.base().record_profile_rows(count);
    }

    /// Record peak memory usage in profile for this operator.
    pub fn record_profile_peak_memory(&self, bytes: u64) {
        if let Some(rt) = &self.base().runtime {
            let node_id = self.plan_node_id();
            let mut profile = rt.profile().lock();
            if let Some(entry) = profile.operators.get_mut(&node_id) {
                if bytes > entry.peak_memory_bytes {
                    entry.peak_memory_bytes = bytes;
                }
            }
        }
    }

    /// Register a resource cleanup callback with the attached runtime.
    pub fn register_resource<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.base().register_resource(f);
    }

    /// Return the base fields.
    pub fn base(&self) -> &OperatorBase {
        match self {
            Self::Source(base, _) => base,
            Self::Unary(base, _, _) => base,
            Self::Join(base, _, _, _)
            | Self::Set(base, _, _, _)
            | Self::Apply(base, _, _, _) => base,
            Self::Blocking(base, _, _) => base,
            Self::Graph(base, _, _) => base,
            Self::Sink(base, _, _) => base,
            Self::Ddl(base, _, _) | Self::Fulltext(base, _, _) | Self::Vector(base, _, _) => base,
            Self::Txn(base, _, _) => base,
        }
    }

    /// Return the base fields (mutable).
    pub fn base_mut(&mut self) -> &mut OperatorBase {
        match self {
            Self::Source(base, _) => base,
            Self::Unary(base, _, _) => base,
            Self::Join(base, _, _, _)
            | Self::Set(base, _, _, _)
            | Self::Apply(base, _, _, _) => base,
            Self::Blocking(base, _, _) => base,
            Self::Graph(base, _, _) => base,
            Self::Sink(base, _, _) => base,
            Self::Ddl(base, _, _) | Self::Fulltext(base, _, _) | Self::Vector(base, _, _) => base,
            Self::Txn(base, _, _) => base,
        }
    }

    /// Return mutable references to all child executors.
    pub fn children_mut(&mut self) -> Vec<&mut Self> {
        match self {
            Self::Source(..) => vec![],
            Self::Unary(_, input, _)
            | Self::Blocking(_, input, _)
            | Self::Graph(_, input, _)
            | Self::Sink(_, input, _)
            | Self::Txn(_, input, _) => vec![input.as_mut()],
            Self::Join(_, left, right, _)
            | Self::Set(_, left, right, _)
            | Self::Apply(_, left, right, _) => vec![left.as_mut(), right.as_mut()],
            Self::Ddl(_, input, _)
            | Self::Fulltext(_, input, _)
            | Self::Vector(_, input, _) => vec![input.as_mut()],
        }
    }

    /// Access the MemoryTracker for blocking/binary operators.
    pub fn memory_tracker(&self) -> Option<&MemoryTracker> {
        match self {
            Self::Source(..)
            | Self::Unary(..)
            | Self::Graph(..)
            | Self::Sink(..)
            | Self::Txn(..)
            | Self::Apply(..)
            | Self::Ddl(..)
            | Self::Fulltext(..)
            | Self::Vector(..) => None,
            Self::Blocking(_, _, op) => Some(op.memory_tracker()),
            Self::Join(_, _, _, op) => Some(op.memory_tracker()),
            Self::Set(_, _, _, op) => Some(op.memory_tracker()),
        }
    }

    /// Whether this operator has been opened.
    pub fn opened(&self) -> bool {
        self.base().opened
    }

    /// Set opened flag.
    pub fn set_opened(&mut self, val: bool) {
        self.base_mut().opened = val;
    }

    // ── Lifecycle dispatch ──

    /// Open the executor.
    pub fn open(&mut self) -> Result<(), QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            Self::Source(base, op) => op.open(base),
            Self::Unary(base, input, op) => op.open(base, input),
            Self::Join(base, left, right, op) => op.open(base, left, right),
            Self::Set(base, left, right, op) => op.open(base, left, right),
            Self::Apply(base, left, right, op) => op.open(base, left, right),
            Self::Blocking(base, input, op) => op.open(base, input),
            Self::Graph(base, input, op) => op.open(base, input),
            Self::Sink(base, input, op) => op.open(base, input),
            Self::Ddl(base, input, op) => op.open(base, input),
            Self::Fulltext(base, input, op) => op.open(base, input),
            Self::Vector(base, input, op) => op.open(base, input),
            Self::Txn(base, input, op) => op.open(base, input),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("open", elapsed);
        result
    }

    /// Pull the next chunk.
    pub fn advance(&mut self) -> Result<Option<DataChunk>, QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            Self::Source(base, op) => op.next(base),
            Self::Unary(base, input, op) => op.next(base, input),
            Self::Join(base, left, right, op) => op.next(base, left, right),
            Self::Set(base, left, right, op) => op.next(base, left, right),
            Self::Apply(base, left, right, op) => op.next(base, left, right),
            Self::Blocking(base, input, op) => op.next(base, input),
            Self::Graph(base, input, op) => op.next(base, input),
            Self::Sink(base, input, op) => op.next(base, input),
            Self::Ddl(base, input, op) => op.next(base, input),
            Self::Fulltext(base, input, op) => op.next(base, input),
            Self::Vector(base, input, op) => op.next(base, input),
            Self::Txn(base, input, op) => op.next(base, input),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        if let Ok(Some(ref chunk)) = result {
            self.record_profile_rows(chunk.len() as u64);
        }
        self.record_profile_timing("next", elapsed);
        result
    }

    /// Stop the executor (signal no more input needed).
    pub fn stop(&mut self) -> Result<(), QueryError> {
        self.ensure_not_cancelled()?;
        let start = Instant::now();
        let result = match self {
            Self::Source(base, op) => op.stop(base),
            Self::Unary(base, input, op) => op.stop(base, input),
            Self::Join(base, left, right, op) => op.stop(base, left, right),
            Self::Set(base, left, right, op) => op.stop(base, left, right),
            Self::Apply(base, left, right, op) => op.stop(base, left, right),
            Self::Blocking(base, input, op) => op.stop(base, input),
            Self::Graph(base, input, op) => op.stop(base, input),
            Self::Sink(base, input, op) => op.stop(base, input),
            Self::Ddl(base, input, op) => op.stop(base, input),
            Self::Fulltext(base, input, op) => op.stop(base, input),
            Self::Vector(base, input, op) => op.stop(base, input),
            Self::Txn(base, input, op) => op.stop(base, input),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("stop", elapsed);
        result
    }

    /// Close the executor (clean up resources).
    pub fn close(&mut self) -> Result<(), QueryError> {
        let start = Instant::now();
        let result = match self {
            Self::Source(base, op) => op.close(base),
            Self::Unary(base, input, op) => op.close(base, input),
            Self::Join(base, left, right, op) => op.close(base, left, right),
            Self::Set(base, left, right, op) => op.close(base, left, right),
            Self::Apply(base, left, right, op) => op.close(base, left, right),
            Self::Blocking(base, input, op) => op.close(base, input),
            Self::Graph(base, input, op) => op.close(base, input),
            Self::Sink(base, input, op) => op.close(base, input),
            Self::Ddl(base, input, op) => op.close(base, input),
            Self::Fulltext(base, input, op) => op.close(base, input),
            Self::Vector(base, input, op) => op.close(base, input),
            Self::Txn(base, input, op) => op.close(base, input),
        };
        let elapsed = start.elapsed().as_micros() as u64;
        self.record_profile_timing("close", elapsed);
        let peak = self.peak_memory_bytes();
        if peak > 0 {
            self.record_profile_peak_memory(peak);
        }
        if let Some(rt) = self.get_runtime() {
            rt.release_resources();
        }
        result
    }
}

impl Spillable for StreamingExecutor {
    fn spill_to_disk(&mut self) -> Result<(), QueryError> {
        Err(QueryError::execution(
            "Disk spill not yet implemented".to_string(),
        ))
    }

    fn spilled_size(&self) -> u64 {
        0
    }
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use crate::core::Value;

    fn create_test_buffer() -> Vec<Vec<Value>> {
        (0..100)
            .map(|i| {
                vec![
                    Value::BigInt(i as i64),
                    Value::String(format!("vertex_{}", i)),
                    Value::String(format!("label_{}", i % 10)),
                    Value::String(format!("prop_{}", i % 100)),
                    Value::BigInt((i % 1000) as i64),
                ]
            })
            .collect()
    }

    fn scan_executor(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
        use super::super::operators::source_operator::SourceOperator;
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                partition_id: 0,
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
    }

    #[test]
    fn test_scan_vertices_with_buffer() {
        let buffer = create_test_buffer();
        let mut executor = scan_executor(buffer.clone(), vec![]);

        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 100);
        executor.close().unwrap();
    }

    #[test]
    fn test_limit_executor() {
        let buffer = create_test_buffer();
        let scan = Box::new(scan_executor(buffer, vec![]));
        let mut executor = StreamingExecutor::Unary(
            OperatorBase::new(0),
            scan,
            UnaryOperator::Limit {
                limit: 10,
                consumed: 0,
            },
        );

        executor.open().unwrap();
        let mut total = 0;
        while let Some(chunk) = executor.advance().unwrap() {
            total += chunk.len();
        }
        executor.close().unwrap();
        assert_eq!(total, 10);
    }

    #[test]
    fn test_dynamic_column_count() {
        let buffer: Vec<Vec<Value>> = vec![
            vec![
                Value::BigInt(1),
                Value::String("a".to_string()),
                Value::String("b".to_string()),
                Value::String("c".to_string()),
                Value::String("d".to_string()),
                Value::String("e".to_string()),
                Value::String("f".to_string()),
                Value::String("g".to_string()),
                Value::String("h".to_string()),
            ],
            vec![
                Value::BigInt(2),
                Value::String("i".to_string()),
                Value::String("j".to_string()),
                Value::String("k".to_string()),
                Value::String("l".to_string()),
                Value::String("m".to_string()),
                Value::String("n".to_string()),
                Value::String("o".to_string()),
                Value::String("p".to_string()),
            ],
        ];

        let mut executor = scan_executor(buffer.clone(), vec![]);
        executor.open().unwrap();
        let chunk = executor.advance().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.len(), 2);
        assert_eq!(chunk.num_columns(), 9);
        executor.close().unwrap();
    }
}

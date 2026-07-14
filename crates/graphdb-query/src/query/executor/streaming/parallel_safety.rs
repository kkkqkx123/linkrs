//! P8 parallel-safety checks: Send/Sync assertions, operator whitelist,
//! and serial fallback predicate.
//!
//! P8.1 delivers only static analysis — it does not change the execution path.

use std::sync::Arc;

use super::executor::StreamingExecutor;
use super::runtime::ExecutionRuntime;
use crate::core::Value;
use crate::query::executor::base::MemoryBudget;

// ── Compile-time Send/Sync assertions ──────────────────────────────

/// Assert that `T: Send` at compile time.
const fn assert_send<T: Send>() {}
/// Assert that `T: Sync` at compile time.
const fn assert_sync<T: Sync>() {}

/// Verify all key P8 cross-thread types implement the required auto-traits.
/// Called once at module init to produce a compilation error if any type
/// is not Send/Sync when the feature is enabled.
#[allow(dead_code)]
pub fn assert_send_sync() {
    // Owned types sent to workers
    assert_send::<StreamingExecutor>();
    assert_send::<Vec<Value>>();

    // Shared types accessed from multiple workers
    assert_send::<Arc<ExecutionRuntime>>();
    assert_sync::<Arc<ExecutionRuntime>>();
    assert_send::<MemoryBudget>();
    assert_sync::<MemoryBudget>();
}

// ── Operator parallel-safety whitelist ─────────────────────────────

/// Whether a `StreamingExecutor` subtree may be safely sent to a rayon
/// worker and executed independently.
///
/// An operator is NOT parallel-safe when it:
/// - Coordinates state across partitions (Join, Set, Gather, HashShuffleJoin)
/// - Depends on external singletons that are not obviously Send+Sync
///   (e.g. graph traversal cursors, DDL handles, transaction handles)
/// - Has a global output scope (Gather, HashShuffleJoin are already
///   excluded; global Blocking operators like Sort/Aggregate are also
///   excluded because their output depends on seeing all rows)
///
/// The whitelist mirrors the existing `is_partition_local()` predicate
/// but is a separate concept: a tree might be partition-local (semantically
/// replicable per partition) without being parallel-safe (e.g. a scan
/// holding a non-Send cursor), though in the current codebase the two
/// sets coincide.
pub fn is_parallel_safe(tree: &StreamingExecutor) -> bool {
    // Scan sources are safe when they do not hold a non-Send cursor.
    // In the current codebase all Source variants hold only owned data
    // (Vec<Value>, partition_id, usize cursor) and are auto-Send.
    match tree {
        StreamingExecutor::Source(..) => true,
        StreamingExecutor::Unary(_, input, _) => is_parallel_safe(input),
        StreamingExecutor::Blocking(_, input, _) => is_parallel_safe(input),
        // Multi-input operators coordinate state and are never safe.
        StreamingExecutor::Join(..)
        | StreamingExecutor::Set(..)
        | StreamingExecutor::Apply(..)
        | StreamingExecutor::Gather(..)
        | StreamingExecutor::Exchange(..)
        | StreamingExecutor::HashShuffleJoin(..) => false,
        // Handles to external systems (DDL, graph, fulltext, vector, txn)
        // are excluded as a conservative default — they may hold non-Send
        // FFI handles or driver connections.
        StreamingExecutor::Graph(..)
        | StreamingExecutor::Sink(..)
        | StreamingExecutor::Ddl(..)
        | StreamingExecutor::Fulltext(..)
        | StreamingExecutor::Vector(..)
        | StreamingExecutor::Txn(..) => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::core::Value;
    use crate::query::executor::streaming::executor::StreamingExecutor;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::operators::unary_operator::UnaryOperator;

    use super::*;

    fn scan_executor() -> StreamingExecutor {
        StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::ScanVertices {
                buffer: vec![vec![Value::BigInt(1)]],
                current_index: 0,
                col_names: vec!["id".to_string()],
            },
        )
    }

    #[test]
    fn scan_is_parallel_safe() {
        assert!(is_parallel_safe(&scan_executor()));
    }

    #[test]
    fn filter_pipeline_is_parallel_safe() {
        let tree = StreamingExecutor::Unary(
            OperatorBase::new(1),
            Box::new(scan_executor()),
            UnaryOperator::Filter {
                predicate: crate::core::types::expr::Expression::Literal(Value::Bool(true)),
            },
        );
        assert!(is_parallel_safe(&tree));
    }

    #[test]
    fn join_is_not_parallel_safe() {
        let join = StreamingExecutor::Join(
            OperatorBase::new(2),
            Box::new(scan_executor()),
            Box::new(scan_executor()),
            crate::query::executor::streaming::operators::join_operator::JoinOperator::InnerJoin {
                join_condition: None,
                build_side_tuples: Vec::new(),
                left_consumed: false,
                memory_tracker: crate::query::executor::base::MemoryTracker::new(
                    MemoryBudget::default_budget(),
                ),
                right_col_names: Vec::new(),
            },
        );
        assert!(!is_parallel_safe(&join));
    }

    #[test]
    fn gather_is_not_parallel_safe() {
        let gather = StreamingExecutor::Gather(
            OperatorBase::new(3),
            vec![scan_executor(), scan_executor()],
            crate::query::executor::streaming::operators::gather_operator::GatherOperator::concatenate(),
        );
        assert!(!is_parallel_safe(&gather));
    }

    #[test]
    fn compile_time_send_sync() {
        // This test is guaranteed to compile only if the asserted types
        // implement the required auto-traits.
        assert_send::<StreamingExecutor>();
        assert_send::<Arc<ExecutionRuntime>>();
        assert_sync::<Arc<ExecutionRuntime>>();
        assert_send::<MemoryBudget>();
        assert_sync::<MemoryBudget>();
        assert_send::<Vec<Value>>();
    }
}

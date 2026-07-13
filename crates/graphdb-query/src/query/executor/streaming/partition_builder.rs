//! Partition-aware executor tree builder.
//!
//! Provides functions for building per-partition executor trees and
//! recursively setting partition information on source operators.

use std::ops::Range;

use super::executor::StreamingExecutor;
use super::operators::source_operator::SourceOperator;
use super::partition::PartitionView;
use crate::core::error::QueryError;
use crate::query::executor::base::ExecutionContext;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

use super::builder::StreamingExecutorBuilder;

/// Build multiple executor trees, one per partition.
///
/// Each tree is identical except for the source operator's partition
/// configuration (partition_id, partition_range).  This enables
/// sequential or parallel processing of partition data.
pub(super) fn build_partitioned(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
    partition_view: &PartitionView,
) -> Result<Vec<StreamingExecutor>, QueryError> {
    let mut executors = Vec::with_capacity(partition_view.partition_count);
    for partition_id in 0..partition_view.partition_count {
        let partition_range = partition_view.get_range(partition_id);
        let mut executor = StreamingExecutorBuilder::from_plan_node(node, context)?;
        set_partition_on_source(&mut executor, partition_id, partition_range)?;
        executors.push(executor);
    }
    Ok(executors)
}

/// Walk the executor tree and set partition info on all source leaves.
fn set_partition_on_source(
    executor: &mut StreamingExecutor,
    partition_id: usize,
    partition_range: Option<Range<i64>>,
) -> Result<(), QueryError> {
    executor.set_partition_id(partition_id);
    match executor {
        StreamingExecutor::Source(_, source) => {
            set_partition_on_source_op(source, partition_id, partition_range);
        }
        StreamingExecutor::Unary(_, input, _)
        | StreamingExecutor::Blocking(_, input, _)
        | StreamingExecutor::Graph(_, input, _)
        | StreamingExecutor::Sink(_, input, _)
        | StreamingExecutor::Ddl(_, input, _)
        | StreamingExecutor::Fulltext(_, input, _)
        | StreamingExecutor::Vector(_, input, _)
        | StreamingExecutor::Txn(_, input, _) => {
            set_partition_on_source(input, partition_id, partition_range)?;
        }
        StreamingExecutor::Join(_, left, right, _)
        | StreamingExecutor::Set(_, left, right, _)
        | StreamingExecutor::Apply(_, left, right, _) => {
            set_partition_on_source(left, partition_id, partition_range.clone())?;
            set_partition_on_source(right, partition_id, partition_range)?;
        }
        StreamingExecutor::Gather(_, children, _) | StreamingExecutor::Exchange(_, children, _) => {
            for child in children.iter_mut() {
                set_partition_on_source(child, partition_id, partition_range.clone())?;
            }
        }
        StreamingExecutor::HashShuffleJoin(_, left, right, _) => {
            for tree in left.iter_mut() {
                set_partition_on_source(tree, partition_id, partition_range.clone())?;
            }
            for tree in right.iter_mut() {
                set_partition_on_source(tree, partition_id, partition_range.clone())?;
            }
        }
    }
    Ok(())
}

/// Set partition info on a source operator.
fn set_partition_on_source_op(
    source: &mut SourceOperator,
    pid: usize,
    prange: Option<Range<i64>>,
) {
    match source {
        SourceOperator::ScanVertices { partition_id, .. } => {
            *partition_id = pid;
        }
        SourceOperator::ScanEdges { partition_id, .. } => {
            *partition_id = pid;
        }
        SourceOperator::StorageScanVertices {
            partition_id,
            partition_range,
            ..
        } => {
            *partition_id = pid;
            *partition_range = prange;
        }
        SourceOperator::StorageScanEdges {
            partition_id,
            partition_range,
            ..
        } => {
            *partition_id = pid;
            *partition_range = prange;
        }
        _ => {}
    }
}

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::columnar::MaterializedBatch;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;
use std::sync::Arc;

pub(super) struct BlockingContext<'a> {
    pub runtime: &'a Option<Arc<ExecutionRuntime>>,
    pub output_layout: &'a Arc<SlotLayout>,
    pub config: &'a OperatorConfig,
}

/// Extract the field name from an aggregate function's args, if any.
/// COUNT(*) has no args; other aggregates have the field expression at index 0.
pub(crate) fn aggregate_arg_field_name(
    func: &AggregateFunction,
    args: &[Expression],
) -> Option<String> {
    match func {
        AggregateFunction::Count => None,
        _ => {
            if let Some(Expression::Variable(name)) = args.first() {
                Some(name.clone())
            } else {
                None
            }
        }
    }
}

/// Shared outlet for blocking operators: replaces the drained
/// `IntoIter<Vec<Value>>` pattern while keeping chunking (2048 rows),
/// logical row order, and outlet observability identical.
pub(super) fn emit_batch_slice(
    batch: &MaterializedBatch,
    offset: &mut usize,
    ctx: &BlockingContext<'_>,
) -> DataChunk {
    let total = batch.num_rows();
    let len = 2048.min(total.saturating_sub(*offset));
    let chunk = DataChunk::slice_from_batch(batch, *offset, len, Arc::clone(ctx.output_layout));
    *offset += len;
    if let Some(rt) = ctx.runtime.as_ref() {
        rt.columnar_stats().record_batch_outlet();
    }
    chunk
}

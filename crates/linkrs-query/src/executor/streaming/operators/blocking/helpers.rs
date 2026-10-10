use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::chunk::TypedColumn;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use linkrs_core::columnar::MaterializedBatch;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use std::sync::Arc;

pub(super) struct BlockingContext<'a> {
    pub runtime: &'a Option<Arc<ExecutionRuntime>>,
    pub output_layout: &'a Arc<SlotLayout>,
    pub config: &'a OperatorConfig,
}

/// Whether a `Count` over a bare variable can skip argument evaluation for
/// this chunk: identity typed columns are provably non-null, and `Count`
/// only observes null-ness, so every visible row counts without evaluating
/// or materializing the argument.
pub(super) fn count_identity_arg(
    func: &AggregateFunction,
    args: &[Expression],
    chunk: &DataChunk,
) -> bool {
    if !matches!(func, AggregateFunction::Count) {
        return false;
    }
    let Some(Expression::Variable(name)) = args.first() else {
        return false;
    };
    let Some(slot) = chunk.get_layout().slot_id(name) else {
        return false;
    };
    matches!(
        chunk.typed_column(slot),
        Some(TypedColumn::VertexIdentity(_)) | Some(TypedColumn::EdgeHeader(_))
    )
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

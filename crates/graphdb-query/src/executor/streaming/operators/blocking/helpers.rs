use crate::executor::base::MemoryTracker;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::spill::SpillManager;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::Value;
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

/// Reject spill for operators that do not support disk-based overflow.
///
/// Unified partition spill for these operators waits for the columnar
/// materialization state (R4 design); until then the memory-budget breach
/// surfaces as an explicit error instead of silent OOM or partial spill.
pub(crate) fn spill_not_supported(
    operator: &'static str,
    _buffer: &mut Vec<Vec<Value>>,
    _sm: &SpillManager,
    _memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    Err(QueryError::execution(
        format!(
            "Spill is not implemented for blocking operator {operator}; query memory budget exceeded (unified spill deferred to columnar materialization state)"
        ),
    ))
}

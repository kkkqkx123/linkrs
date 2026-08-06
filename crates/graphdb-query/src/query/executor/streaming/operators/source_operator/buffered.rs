use crate::core::error::QueryError;
use crate::core::Value;
use crate::query::executor::expression::evaluation_context::default_context::DefaultExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::state::GlobalState;

use super::util::{attach_columnar_stats, reserve_memory};
use super::SourceOperator;

/// Open the buffered source variants.
///
/// `ScanVertices`/`ScanEdges` reset their row index; `StandaloneValues`
/// evaluates its expressions once so volatile values (e.g. `now()`) are
/// resolved at execution time.
pub(crate) fn open(op: &mut SourceOperator, base: &mut OperatorBase) -> Result<(), QueryError> {
    match op {
        SourceOperator::ScanVertices {
            current_index,
            col_names,
            ..
        } => {
            *current_index = 0;
            base.insert_state(GlobalState::Source(SourceState::ScanVertices {
                current_index: 0,
                col_names: col_names.clone(),
            }));
        }
        SourceOperator::StandaloneValues {
            values,
            buffer,
            current_index,
            col_names,
        } => {
            let mut context = DefaultExpressionContext::new();
            if let Some(params) = base.runtime.as_ref().and_then(|rt| rt.parameter_values()) {
                context = context.with_parameters(params);
            }
            let mut rows = Vec::with_capacity(values.len());
            for row in values {
                let mut evaluated = Vec::with_capacity(row.len());
                for expr in row {
                    let expression = expr.get_expression().ok_or_else(|| {
                        QueryError::execution(
                            "StandaloneValues expression not found in context".to_string(),
                        )
                    })?;
                    evaluated.push(
                        ExpressionEvaluator::evaluate(&expression, &mut context).map_err(
                            |error| {
                                QueryError::execution(format!(
                                    "StandaloneValues expression evaluation failed: {error}"
                                ))
                            },
                        )?,
                    );
                }
                rows.push(evaluated);
            }
            *buffer = rows;
            *current_index = 0;
            base.insert_state(GlobalState::Source(SourceState::ScanVertices {
                current_index: 0,
                col_names: col_names.clone(),
            }));
        }
        SourceOperator::ScanEdges {
            current_index,
            col_names,
            ..
        } => {
            *current_index = 0;
            base.insert_state(GlobalState::Source(SourceState::ScanEdges {
                current_index: 0,
                col_names: col_names.clone(),
            }));
        }
        _ => unreachable!("buffered::open called for a non-buffered source"),
    }
    Ok(())
}

/// Emit the next chunk for the buffered source variants.
pub(crate) fn next(
    op: &mut SourceOperator,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    match op {
        SourceOperator::ScanVertices {
            buffer,
            current_index,
            col_names,
            ..
        }
        | SourceOperator::StandaloneValues {
            buffer,
            current_index,
            col_names,
            ..
        }
        | SourceOperator::ScanEdges {
            buffer,
            current_index,
            col_names,
            ..
        } => next_buffer_chunk(base, buffer, current_index, col_names),
        _ => unreachable!("buffered next called for a non-buffered source"),
    }
}

fn next_buffer_chunk(
    base: &OperatorBase,
    buffer: &[Vec<Value>],
    current_index: &mut usize,
    col_names: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    if *current_index >= buffer.len() {
        return Ok(None);
    }
    let end = (*current_index + base.chunk_size).min(buffer.len());
    let rows = buffer[*current_index..end].to_vec();
    *current_index = end;
    let reservation = reserve_memory(base, &rows)?;
    let layout = if col_names.is_empty() {
        let width = rows.first().map_or(0, Vec::len);
        let inferred: Vec<String> = (0..width).map(|i| format!("c{i}")).collect();
        std::sync::Arc::new(SlotLayout::from_names(&inferred))
    } else {
        std::sync::Arc::new(SlotLayout::from_names(col_names))
    };
    let chunk = DataChunk::new_with_layout(rows, layout);
    let chunk = attach_columnar_stats(base, chunk);
    let chunk = if let Some(reservation) = reservation {
        chunk.with_memory_reservation(reservation)
    } else {
        chunk
    };
    Ok(Some(chunk))
}

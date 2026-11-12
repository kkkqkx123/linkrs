use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use linkrs_core::error::QueryError;
#[cfg(feature = "vector")]
use linkrs_core::Value;

use super::{VectorOperator, VectorOperatorKind};

pub(crate) fn handle(op: &mut VectorOperator, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match &mut op.kind {
        VectorOperatorKind::VectorSearch {
            storage: _,
            space_name: _,
            space_id,
            index_name: _,
            query_vector,
            query_text,
            top_k,
            tag_name,
            field_name,
            threshold,
            filter,
            offset,
            #[cfg(feature = "vector")]
            ryw_config,
            #[cfg(feature = "vector")]
            vector_coordinator,
            ..
        } => {
            #[cfg(feature = "vector")]
            {
                if let Some(coordinator) = vector_coordinator {
                    // The planner resolves index_name to (tag, field)
                    // via the index registry; an empty location is an
                    // unknown index, not an empty match set.
                    if tag_name.is_empty() || field_name.is_empty() {
                        return Err(QueryError::execution(
                            "SEARCH VECTOR cannot execute: index location (tag/field) is not resolved",
                        ));
                    }
                    let resolved_vector = if let Some(text) = query_text {
                        let _ = text;
                        return Err(QueryError::execution(
                            "TEXT vector query requires pre-vectorized input; vectorize upstream",
                        ));
                    } else {
                        query_vector.clone()
                    };
                    let recall_limit = (*top_k as usize).saturating_add(*offset);
                    let mut options = linkrs_sync::vector_sync::SearchOptions::new(
                        *space_id,
                        tag_name.clone(),
                        field_name.clone(),
                        resolved_vector,
                        recall_limit,
                    );
                    match threshold {
                        Some(t) if *t > 0.0 => options.threshold = Some(*t),
                        _ => {}
                    }
                    if let Some(filter) = filter {
                        options.filter = Some(filter.clone());
                    }
                    let _ = ryw_config;
                    let search_results =
                        crate::executor::streaming::helpers::runtime_bridge::wait(
                            "Vector search",
                            coordinator.index_manager().search_with_options(options),
                        )?;
                    let mut rows = Vec::new();
                    for result in search_results.into_iter().skip(*offset) {
                        rows.push(vec![
                            Value::string(result.id.to_string()),
                            Value::Double(result.score as f64),
                        ]);
                    }
                    return if rows.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(DataChunk::new_with_layout(
                            rows,
                            op.output_layout.clone(),
                        )))
                    };
                }

                // A missing coordinator is a configuration error, not an
                // empty result: fail loudly instead of silently returning
                // nothing (or unrelated input rows).
                let _ = input;
                Err(QueryError::execution(
                    "SEARCH VECTOR cannot execute: no vector coordinator is configured",
                ))
            }

            #[cfg(not(feature = "vector"))]
            {
                let _ = (
                    &space_id,
                    &tag_name,
                    &field_name,
                    &query_vector,
                    query_text.as_deref(),
                    &top_k,
                    &threshold,
                    &filter,
                    &offset,
                    input,
                );
                Err(QueryError::feature_disabled("vector", "VECTOR SEARCH"))
            }
        }
        _ => unreachable!("vector_operator::search called for an unknown kind"),
    }
}

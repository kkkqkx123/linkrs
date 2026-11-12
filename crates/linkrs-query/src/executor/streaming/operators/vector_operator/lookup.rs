use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use linkrs_core::error::QueryError;
#[cfg(feature = "vector")]
use linkrs_core::Value;

use super::{VectorOperator, VectorOperatorKind};

pub(crate) fn handle(op: &mut VectorOperator, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match &mut op.kind {
        VectorOperatorKind::VectorLookup {
            storage: _,
            space_name: _,
            space_id,
            index_name: _,
            query_vector,
            query_text,
            top_k,
            tag_name,
            field_name,
            #[cfg(feature = "vector")]
            ryw_config,
            #[cfg(feature = "vector")]
            vector_coordinator,
            ..
        } => {
            #[cfg(feature = "vector")]
            {
                if let Some(coordinator) = vector_coordinator {
                    // LOOKUP VECTOR resolves to the same index location
                    // as SEARCH VECTOR and reuses the identical search
                    // path, producing (id, score) rows.
                    if tag_name.is_empty() || field_name.is_empty() {
                        return Err(QueryError::execution(
                            "LOOKUP VECTOR cannot execute: index location (tag/field) is not resolved",
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
                    let options = linkrs_sync::vector_sync::SearchOptions::new(
                        *space_id,
                        tag_name.clone(),
                        field_name.clone(),
                        resolved_vector,
                        *top_k as usize,
                    );
                    let _ = ryw_config;
                    let search_results =
                        crate::executor::streaming::helpers::runtime_bridge::wait(
                            "Vector lookup",
                            coordinator.index_manager().search_with_options(options),
                        )?;
                    let mut rows = Vec::new();
                    for result in search_results {
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

                let _ = input;
                Err(QueryError::execution(
                    "LOOKUP VECTOR cannot execute: no vector coordinator is configured",
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
                    input,
                );
                Err(QueryError::feature_disabled("vector", "VECTOR LOOKUP"))
            }
        }
        _ => unreachable!("vector_operator::lookup called for an unknown kind"),
    }
}

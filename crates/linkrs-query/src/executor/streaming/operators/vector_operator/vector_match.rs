use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use linkrs_core::error::QueryError;
#[cfg(feature = "vector")]
use linkrs_core::Value;

use super::{VectorOperator, VectorOperatorKind};

/// The `MATCH VECTOR` grammar has no LIMIT clause yet (see the parser AST),
/// so a fixed candidate window is used; wire this to syntax once LIMIT is
/// added to the statement form. Planners and hybrid fusion recall depths
/// must reference this single source instead of copying the value.
///
/// Hybrid retrieval is not wired into this operator: the unified fusion
/// entry point is `crate::fusion::HybridFusionRequest`, which normalizes
/// vector point IDs before fusing. Callers that need fulltext + vector
/// fusion must go through that entry (or `fuse_fulltext_and_vector`)
/// instead of reimplementing ID normalization here or at the call site.
#[cfg(feature = "vector")]
pub(crate) const DEFAULT_MATCH_TOP_K: usize = 100;

pub(crate) fn handle(op: &mut VectorOperator, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
    match &mut op.kind {
        VectorOperatorKind::VectorMatch {
            storage: _,
            space_name: _,
            pattern: _,
            field: _,
            query_vector,
            query_text,
            threshold,
            tag_name,
            field_name,
            space_id,
            #[cfg(feature = "vector")]
            ryw_config,
            #[cfg(feature = "vector")]
            vector_coordinator,
            ..
        } => {
            #[cfg(feature = "vector")]
            {
                if let Some(coordinator) = vector_coordinator {
                    let resolved_vector = if let Some(text) = query_text {
                        let _ = text;
                        return Err(QueryError::execution(
                            "TEXT vector query requires pre-vectorized input; vectorize upstream",
                        ));
                    } else {
                        query_vector.clone()
                    };
                    let thr = threshold.unwrap_or(0.5);
                    let options = linkrs_sync::vector_sync::SearchOptions::new(
                        *space_id,
                        tag_name.clone(),
                        field_name.clone(),
                        resolved_vector,
                        DEFAULT_MATCH_TOP_K,
                    )
                    .with_threshold(thr);
                    let _ = ryw_config;
                    let search_results =
                        crate::executor::streaming::helpers::runtime_bridge::wait(
                            "Vector match",
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
                    "MATCH VECTOR cannot execute: no vector coordinator is configured",
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
                    &threshold,
                    input,
                );
                Err(QueryError::feature_disabled("vector", "VECTOR MATCH"))
            }
        }
        _ => unreachable!("vector_operator::vector_match called for an unknown kind"),
    }
}

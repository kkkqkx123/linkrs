//! Hybrid fulltext + vector retrieval for an embedded session.
//!
//! Runs both secondary-index branches against the same `(tag, field)` index
//! and fuses them through the validated `HybridFusionRequest` contract.
//! Vector point ID normalization lives inside the fusion entry; this module
//! only converts each backend's native result type into `SearchHit`.

use super::Session;
use crate::api_core::{CoreError, CoreResult};
use linkrs_core::types::search::SearchHit;
use linkrs_query::HybridFusionRequest;
use linkrs_storage::StorageClient;

#[cfg(all(feature = "fulltext", feature = "vector"))]
impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Hybrid fulltext + vector search fused by the validated contract.
    ///
    /// # Parameters
    /// - `request` - fusion contract (branch limits, strategy, yield fields)
    /// - `query_text` - fulltext query string for the fulltext branch
    /// - `query_vector` - query embedding for the vector branch
    ///
    /// # Return
    /// - Returns fused hits sorted by fused score descending, truncated to
    ///   the contract's `fused_limit`
    /// - Return error on failure
    pub async fn hybrid_search(
        &self,
        request: HybridFusionRequest,
        query_text: &str,
        query_vector: Vec<f32>,
    ) -> CoreResult<Vec<SearchHit>> {
        let recall_depth = request.per_branch_limit;
        let fulltext_results = self
            .search_fulltext(&request.tag, &request.field, query_text, recall_depth)
            .await?;
        let vector_results = self
            .simvec(&request.tag, &request.field, query_vector, recall_depth)
            .await?;

        let fulltext_hits = fulltext_results
            .into_iter()
            .map(|r| {
                let id = r.doc_id.string_value().ok_or_else(|| {
                    CoreError::InvalidParameter("fulltext doc_id is not a string value".to_string())
                })?;
                let mut hit = SearchHit::new(id, r.score);
                hit.highlights = r.highlights;
                Ok(hit)
            })
            .collect::<CoreResult<Vec<_>>>()?;

        let vector_hits = vector_results
            .into_iter()
            .map(linkrs_core::types::search::IntoSearchHit::into_search_hit)
            .collect();

        request
            .execute(fulltext_hits, vector_hits)
            .map_err(CoreError::InvalidParameter)
    }
}

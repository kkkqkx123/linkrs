//! Operator-level hybrid retrieval fusion.
//!
//! The fulltext and vector search paths each emit `(id, score)` rows with
//! different identifier spaces: fulltext vertex docs carry the raw vertex
//! display form while vector point IDs use a `{vertex}#tag#field` encoding.
//! Raw fusion helpers alone (`rrf_fuse` / `weighted_fuse`) rely on every
//! call site remembering to normalize vector IDs first; the entry point in
//! this module encapsulates that step so hybrid search cannot skip it.
//!
//! Parser status: no `SEARCH HYBRID` statement exists; fulltext and vector
//! remain independent planning paths. Hybrid fusion enters the main chain
//! only through [`HybridFusionRequest::execute`] (operator-level, validated
//! contract below), not through parser syntax or app-layer helpers.

use linkrs_core::fusion::{normalize_vector_hits, rrf_fuse, weighted_fuse};
use linkrs_core::types::search::SearchHit;

/// Fusion strategy for a hybrid fulltext + vector search.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FusionStrategy {
    /// Reciprocal rank fusion with the standard rank constant.
    Rrf { k: u32 },
    /// Weighted blend of min-max normalized scores: `alpha * vector +
    /// (1 - alpha) * fulltext`.
    Weighted { alpha: f32 },
}

impl Default for FusionStrategy {
    fn default() -> Self {
        FusionStrategy::Rrf { k: 60 }
    }
}

/// Validated contract for one hybrid fulltext + vector fusion.
///
/// Semantics (step 1 of the hybrid wiring):
/// - Pairing: both branches must target the same `(tag, field)` vector index;
///   vector hits outside that index are dropped during normalization, edge
///   fulltext docs pass through for caller-side filtering.
/// - Recall vs return: `per_branch_limit` caps each input list (recall
///   depth); `fused_limit` caps the fused output. Both must be `> 0` and
///   `per_branch_limit >= fused_limit`; `limit == 0` is rejected.
/// - Strategy params: `Rrf{k}` requires `1 <= k <= 1000` (default 60);
///   `Weighted{alpha}` requires `0.0 <= alpha <= 1.0` (rejected, not
///   clamped, so misconfiguration surfaces instead of silently shifting).
/// - Fused rows carry `SearchHit{id, score}` plus merged `highlights`,
///   `payload`, and `source`: the fulltext branch is the base carrier for
///   highlights/source, and payload keys are unioned with fulltext values
///   taking precedence over vector-only additions. Requested
///   `yield_fields` must be non-empty and resolvable on both indexes. Field
///   existence is enforced at planning; this layer only enforces
///   non-emptiness so an unconstrained fusion cannot enter the chain.
#[derive(Debug, Clone, PartialEq)]
pub struct HybridFusionRequest {
    pub tag: String,
    pub field: String,
    pub strategy: FusionStrategy,
    pub per_branch_limit: usize,
    pub fused_limit: usize,
    pub yield_fields: Vec<String>,
}

impl HybridFusionRequest {
    pub fn new(
        tag: impl Into<String>,
        field: impl Into<String>,
        strategy: FusionStrategy,
        per_branch_limit: usize,
        fused_limit: usize,
        yield_fields: Vec<String>,
    ) -> Self {
        Self {
            tag: tag.into(),
            field: field.into(),
            strategy,
            per_branch_limit,
            fused_limit,
            yield_fields,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.tag.is_empty() {
            return Err("hybrid fusion requires a non-empty tag".to_string());
        }
        if self.field.is_empty() {
            return Err("hybrid fusion requires a non-empty field".to_string());
        }
        if self.per_branch_limit == 0 {
            return Err("hybrid fusion per_branch_limit must be > 0".to_string());
        }
        if self.fused_limit == 0 {
            return Err("hybrid fusion fused_limit must be > 0".to_string());
        }
        if self.per_branch_limit < self.fused_limit {
            return Err(format!(
                "hybrid fusion per_branch_limit ({}) must be >= fused_limit ({})",
                self.per_branch_limit, self.fused_limit
            ));
        }
        match self.strategy {
            FusionStrategy::Rrf { k } => {
                if !(1..=1000).contains(&k) {
                    return Err(format!(
                        "hybrid fusion Rrf k must be in 1..=1000, got {k}"
                    ));
                }
            }
            FusionStrategy::Weighted { alpha } => {
                if !(0.0..=1.0).contains(&alpha) {
                    return Err(format!(
                        "hybrid fusion Weighted alpha must be in [0,1], got {alpha}"
                    ));
                }
            }
        }
        if self.yield_fields.is_empty() {
            return Err("hybrid fusion requires non-empty yield_fields".to_string());
        }
        Ok(())
    }

    /// Execute the validated fusion over two retrieved hit lists.
    ///
    /// Main-chain entry: truncates each branch to `per_branch_limit`,
    /// normalizes vector IDs for `(tag, field)`, fuses under the strategy,
    /// and truncates to `fused_limit`.
    pub fn execute(
        &self,
        fulltext: Vec<SearchHit>,
        vector: Vec<SearchHit>,
    ) -> Result<Vec<SearchHit>, String> {
        self.validate()?;
        let mut fulltext = fulltext;
        let mut vector = vector;
        fulltext.truncate(self.per_branch_limit);
        vector.truncate(self.per_branch_limit);
        Ok(fuse_fulltext_and_vector(
            fulltext,
            vector,
            &self.tag,
            &self.field,
            self.strategy,
            self.fused_limit,
        ))
    }
}

/// Fuse one fulltext hit list with one vector hit list.
///
/// Vector point IDs are first normalized to the fulltext document key space
/// for the requested `(tag, field)` index; hits that do not belong to that
/// index are dropped. The lists then fuse under the given strategy and the
/// result is sorted by fused score descending, truncated to `limit`.
pub fn fuse_fulltext_and_vector(
    fulltext: Vec<SearchHit>,
    vector: Vec<SearchHit>,
    tag: &str,
    field: &str,
    strategy: FusionStrategy,
    limit: usize,
) -> Vec<SearchHit> {
    let vector = normalize_vector_hits(&vector, tag, field);
    match strategy {
        FusionStrategy::Rrf { k } => rrf_fuse(&[fulltext, vector], k, limit),
        FusionStrategy::Weighted { alpha } => weighted_fuse(&vector, &fulltext, alpha, limit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_fusion_normalizes_vector_ids_before_fusing() {
        // The vector point encodes the same vertex the fulltext doc names;
        // `#` in the vertex display form is percent-encoded (`%23`), so the
        // two lists only recognize one hit after normalization.
        let fulltext = vec![SearchHit::new("vertex-1#Person", 3.0)];
        let vector = vec![SearchHit::new(
            "vertex-1%23Person#Person#embedding",
            0.9,
        )];
        let fused = fuse_fulltext_and_vector(
            fulltext,
            vector,
            "Person",
            "embedding",
            FusionStrategy::Rrf { k: 60 },
            10,
        );
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "vertex-1#Person");
    }

    #[test]
    fn foreign_vector_hits_are_dropped() {
        let fulltext = vec![SearchHit::new("vertex-1", 3.0)];
        let vector = vec![SearchHit::new("vertex-1#OtherTag#other", 0.9)];
        let fused = fuse_fulltext_and_vector(
            fulltext,
            vector,
            "Person",
            "embedding",
            FusionStrategy::default(),
            10,
        );
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "vertex-1");
    }

    #[test]
    fn weighted_fusion_blends_normalized_scores() {
        let fulltext = vec![SearchHit::new("vertex-1", 3.0)];
        let vector = vec![SearchHit::new("vertex-1#Person#embedding", 1.0)];
        let fused = fuse_fulltext_and_vector(
            fulltext,
            vector,
            "Person",
            "embedding",
            FusionStrategy::Weighted { alpha: 0.5 },
            10,
        );
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "vertex-1");
        // Both lists are single-element (normalize to 1.0), so the blend is
        // alpha * 1.0 + (1 - alpha) * 1.0 = 1.0.
        assert!((fused[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn hybrid_request_validates_contract() {
        let good = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::Rrf { k: 60 },
            10,
            5,
            vec!["name".to_string()],
        );
        assert!(good.validate().is_ok());

        let bad_k = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::Rrf { k: 0 },
            10,
            5,
            vec!["name".to_string()],
        );
        assert!(bad_k.validate().is_err());

        let bad_alpha = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::Weighted { alpha: 1.5 },
            10,
            5,
            vec!["name".to_string()],
        );
        assert!(bad_alpha.validate().is_err());

        let bad_limits = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::default(),
            3,
            5,
            vec!["name".to_string()],
        );
        assert!(bad_limits.validate().is_err());

        let no_yield = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::default(),
            10,
            5,
            vec![],
        );
        assert!(no_yield.validate().is_err());
    }

    #[test]
    fn hybrid_request_end_to_end_fuses_both_branches() {
        let request = HybridFusionRequest::new(
            "Person",
            "embedding",
            FusionStrategy::Rrf { k: 60 },
            10,
            5,
            vec!["name".to_string()],
        );
        let fulltext = vec![
            SearchHit::new("vertex-1", 3.0),
            SearchHit::new("vertex-2", 2.0),
        ];
        let vector = vec![
            SearchHit::new("vertex-2#Person#embedding", 0.9),
            SearchHit::new("vertex-3#Person#embedding", 0.8),
        ];
        let fused = request.execute(fulltext, vector).expect("contract holds");
        assert!(!fused.is_empty());
        assert!(fused.len() <= 5);
        assert!(
            fused.iter().any(|hit| hit.id == "vertex-2"),
            "vertex present in both branches must survive fusion"
        );
    }
}

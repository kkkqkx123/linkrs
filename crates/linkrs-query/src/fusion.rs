//! Operator-level hybrid retrieval fusion.
//!
//! The fulltext and vector search paths each emit `(id, score)` rows with
//! different identifier spaces: fulltext vertex docs carry the raw vertex
//! display form while vector point IDs use a `{vertex}#tag#field` encoding.
//! Raw fusion helpers alone (`rrf_fuse` / `weighted_fuse`) rely on every
//! call site remembering to normalize vector IDs first; the entry point in
//! this module encapsulates that step so hybrid search cannot skip it.

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
}

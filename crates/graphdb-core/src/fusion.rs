//! Application-layer fusion for fulltext and vector results.
//!
//! Both search paths emit `(document id, score)` rows independently. This
//! module merges those rows without touching the existing planners or
//! operators. Rank fusion needs no score calibration; weighted fusion
//! normalizes each input list before combining.

use std::collections::HashMap;

/// One ranked hit from a single retrieval path.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredHit {
    pub id: String,
    pub score: f32,
}

impl ScoredHit {
    pub fn new(id: impl Into<String>, score: f32) -> Self {
        Self {
            id: id.into(),
            score,
        }
    }
}

/// Fused hit shared by both strategies.
#[derive(Debug, Clone, PartialEq)]
pub struct FusedHit {
    pub id: String,
    pub score: f32,
}

/// Fuse ranked lists with reciprocal rank fusion.
///
/// Each hit contributes `1 / (k + rank)` per list it appears in, with
/// 1-based ranks. Output is sorted by fused score descending and truncated
/// to `limit` entries.
pub fn rrf_fuse(lists: &[Vec<ScoredHit>], k: u32, limit: usize) -> Vec<FusedHit> {
    let shift = k as f32;
    let mut totals: HashMap<&str, f32> = HashMap::new();
    for list in lists {
        for (position, hit) in list.iter().enumerate() {
            let rank = position as f32 + 1.0;
            let entry = totals.entry(hit.id.as_str()).or_insert(0.0);
            *entry += 1.0 / (shift + rank);
        }
    }
    let mut fused: Vec<FusedHit> = totals
        .into_iter()
        .map(|(id, score)| FusedHit {
            id: id.to_string(),
            score,
        })
        .collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score));
    fused.truncate(limit);
    fused
}

/// Fuse one vector list with one fulltext list by weighted sum.
///
/// Each list is min-max normalized into `0..=1` before combining as
/// `alpha * vector + (1 - alpha) * fulltext`. Degenerate lists where all
/// scores are equal normalize to `1.0` so presence still counts.
pub fn weighted_fuse(
    vector: &[ScoredHit],
    fulltext: &[ScoredHit],
    alpha: f32,
    limit: usize,
) -> Vec<FusedHit> {
    let weight = alpha.clamp(0.0, 1.0);
    let vector_norm = normalize(vector);
    let fulltext_norm = normalize(fulltext);
    let mut totals: HashMap<&str, f32> = HashMap::new();
    for (id, score) in &vector_norm {
        let entry = totals.entry(id.as_str()).or_insert(0.0);
        *entry += weight * score;
    }
    for (id, score) in &fulltext_norm {
        let entry = totals.entry(id.as_str()).or_insert(0.0);
        *entry += (1.0 - weight) * score;
    }
    let mut fused: Vec<FusedHit> = totals
        .into_iter()
        .map(|(id, score)| FusedHit {
            id: id.to_string(),
            score,
        })
        .collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score));
    fused.truncate(limit);
    fused
}

fn normalize(hits: &[ScoredHit]) -> Vec<(String, f32)> {
    if hits.is_empty() {
        return Vec::new();
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for hit in hits {
        if hit.score < min {
            min = hit.score;
        }
        if hit.score > max {
            max = hit.score;
        }
    }
    let span = max - min;
    hits.iter()
        .map(|hit| {
            let normalized = if span <= f32::EPSILON {
                1.0
            } else {
                (hit.score - min) / span
            };
            (hit.id.clone(), normalized)
        })
        .collect()
}

/// Normalize a vector point ID to its vertex document key for fusion.
///
/// Vector points use `{vertex}#tag#field` with `%` escaped as `%25` and `#`
/// as `%23`. Fulltext vertex docs use the raw vertex display form, so callers
/// must map both lists through this helper before `rrf_fuse` or
/// `weighted_fuse`. Edge fulltext docs (`src->dst#ranking`) never match
/// vector points; they pass through unchanged for caller-side filtering.
pub fn normalize_vector_point_id(point_id: &str, tag: &str, field: &str) -> String {
    let suffix = format!("#{}#{}", tag, field);
    let encoded = point_id.strip_suffix(&suffix).unwrap_or(point_id);
    encoded.replace("%23", "#").replace("%25", "%")
}

/// Map one vector hit list to fusion keys, dropping hits that do not belong
/// to the requested index.
pub fn normalize_vector_hits(hits: &[ScoredHit], tag: &str, field: &str) -> Vec<ScoredHit> {
    let suffix = format!("#{}#{}", tag, field);
    hits.iter()
        .filter(|hit| hit.id.ends_with(&suffix))
        .map(|hit| ScoredHit::new(normalize_vector_point_id(&hit.id, tag, field), hit.score))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_prefers_hits_present_in_both_lists() {
        let vector = vec![ScoredHit::new("a", 0.9), ScoredHit::new("b", 0.5)];
        let fulltext = vec![ScoredHit::new("b", 2.0), ScoredHit::new("c", 1.0)];
        let fused = rrf_fuse(&[vector, fulltext], 60, 10);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].id, "b");
    }

    #[test]
    fn rrf_respects_limit() {
        let first = vec![ScoredHit::new("a", 1.0), ScoredHit::new("b", 0.5)];
        let fused = rrf_fuse(&[first], 60, 1);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "a");
    }

    #[test]
    fn weighted_fuse_blends_both_sources() {
        let vector = vec![ScoredHit::new("a", 1.0), ScoredHit::new("b", 0.0)];
        let fulltext = vec![ScoredHit::new("b", 5.0), ScoredHit::new("c", 1.0)];
        let fused = weighted_fuse(&vector, &fulltext, 0.5, 10);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].id, "b");
    }

    #[test]
    fn weighted_fuse_handles_degenerate_lists() {
        let vector = vec![ScoredHit::new("a", 2.0)];
        let fulltext = vec![ScoredHit::new("a", 7.0)];
        let fused = weighted_fuse(&vector, &fulltext, 0.5, 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "a");
        assert!((fused[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vector_point_id_normalization_strips_suffix() {
        let normalized = normalize_vector_point_id("v1#Tag#bio", "Tag", "bio");
        assert_eq!(normalized, "v1");
        let escaped = normalize_vector_point_id("a%23b#Tag#bio", "Tag", "bio");
        assert_eq!(escaped, "a#b");
    }
}

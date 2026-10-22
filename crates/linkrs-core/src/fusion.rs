//! Application-layer fusion for fulltext and vector results.
//!
//! Both search paths emit [`SearchHit`] rows independently. This module
//! merges those rows without touching the existing planners or operators.
//! Rank fusion needs no score calibration; weighted fusion normalizes each
//! input list before combining.

use crate::types::search::SearchHit;
use std::collections::HashMap;

/// Fuse ranked lists with reciprocal rank fusion.
///
/// Each hit contributes `1 / (k + rank)` per list it appears in, with
/// 1-based ranks. Output is sorted by fused score descending and truncated
/// to `limit` entries.
pub fn rrf_fuse(lists: &[Vec<SearchHit>], k: u32, limit: usize) -> Vec<SearchHit> {
    let shift = k as f32;
    // Track every contributing hit per id so highlight/payload/source can be
    // merged instead of silently dropping the non-base branch's data.
    let mut totals: HashMap<String, (f32, Vec<&SearchHit>)> = HashMap::new();
    for list in lists {
        for (position, hit) in list.iter().enumerate() {
            let rank = position as f32 + 1.0;
            let entry = totals
                .entry(hit.id.clone())
                .or_insert_with(|| (0.0, Vec::new()));
            entry.0 += 1.0 / (shift + rank);
            entry.1.push(hit);
        }
    }
    let mut fused: Vec<SearchHit> = totals
        .into_iter()
        .map(|(id, (score, hits))| merge_hits(&id, score, &hits))
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
    vector: &[SearchHit],
    fulltext: &[SearchHit],
    alpha: f32,
    limit: usize,
) -> Vec<SearchHit> {
    let weight = alpha.clamp(0.0, 1.0);
    let vector_norm = normalize(vector);
    let fulltext_norm = normalize(fulltext);

    // Collect the contributing hit references per id, preferring fulltext as
    // the base carrier but retaining vector-only payload.
    let mut by_id: HashMap<String, (Option<&SearchHit>, Option<&SearchHit>)> = HashMap::new();
    for hit in vector {
        by_id.entry(hit.id.clone()).or_default().0 = Some(hit);
    }
    for hit in fulltext {
        by_id.entry(hit.id.clone()).or_default().1 = Some(hit);
    }

    let mut fused: Vec<SearchHit> = by_id
        .into_iter()
        .map(|(id, (v_hit, f_hit))| {
            let vscore = vector_norm
                .iter()
                .find(|(hid, _)| hid == &id)
                .map(|(_, s)| *s)
                .unwrap_or(0.0);
            let fscore = fulltext_norm
                .iter()
                .find(|(hid, _)| hid == &id)
                .map(|(_, s)| *s)
                .unwrap_or(0.0);
            let score = weight * vscore + (1.0 - weight) * fscore;
            let mut hits: Vec<&SearchHit> = Vec::new();
            if let Some(h) = f_hit {
                hits.push(h);
            }
            if let Some(h) = v_hit {
                hits.push(h);
            }
            if hits.is_empty() {
                // Should not happen: by_id only contains seen hits.
                return SearchHit::new(id, score);
            }
            merge_hits(&id, score, &hits)
        })
        .collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score));
    fused.truncate(limit);
    fused
}

/// Merge highlight/payload/source data from multiple contributing hits into
/// one fused row.
///
/// The first hit in `hits` is the base carrier (fulltext first in the
/// hybrid chain). Highlights, source, and any payload keys already present
/// on the base are preserved; payload keys present only on later hits (e.g.
/// vector-only fields) are added without overwriting existing values.
fn merge_hits(id: &str, score: f32, hits: &[&SearchHit]) -> SearchHit {
    let mut highlights = None;
    let mut payload: Option<HashMap<String, crate::Value>> = None;
    let mut source = None;
    for hit in hits {
        if highlights.is_none() {
            highlights = hit.highlights.clone();
        }
        if source.is_none() {
            source = hit.source.clone();
        }
        if let Some(hp) = &hit.payload {
            match payload.as_mut() {
                Some(p) => {
                    for (k, v) in hp {
                        p.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
                None => {
                    payload = Some(hp.clone());
                }
            }
        }
    }
    SearchHit {
        id: id.to_string(),
        score,
        highlights,
        payload,
        source,
    }
}

fn normalize(hits: &[SearchHit]) -> Vec<(String, f32)> {
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
pub fn normalize_vector_hits(hits: &[SearchHit], tag: &str, field: &str) -> Vec<SearchHit> {
    let suffix = format!("#{}#{}", tag, field);
    hits.iter()
        .filter(|hit| hit.id.ends_with(&suffix))
        .map(|hit| {
            let normalized_id = normalize_vector_point_id(&hit.id, tag, field);
            SearchHit {
                id: normalized_id,
                ..hit.clone()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, score: f32) -> SearchHit {
        SearchHit::new(id, score)
    }

    #[test]
    fn rrf_prefers_hits_present_in_both_lists() {
        let vector = vec![hit("a", 0.9), hit("b", 0.5)];
        let fulltext = vec![hit("b", 2.0), hit("c", 1.0)];
        let fused = rrf_fuse(&[vector, fulltext], 60, 10);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].id, "b");
    }

    #[test]
    fn rrf_respects_limit() {
        let first = vec![hit("a", 1.0), hit("b", 0.5)];
        let fused = rrf_fuse(&[first], 60, 1);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "a");
    }

    #[test]
    fn weighted_fuse_blends_both_sources() {
        let vector = vec![hit("a", 1.0), hit("b", 0.0)];
        let fulltext = vec![hit("b", 5.0), hit("c", 1.0)];
        let fused = weighted_fuse(&vector, &fulltext, 0.3, 10);
        assert_eq!(fused.len(), 3);
        assert_eq!(fused[0].id, "b");
    }

    #[test]
    fn weighted_fuse_handles_degenerate_lists() {
        let vector = vec![hit("a", 2.0)];
        let fulltext = vec![hit("a", 7.0)];
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
    #[test]
    fn rrf_merges_payload_from_vector_into_fulltext_base() {
        use std::collections::HashMap;
        let mut vpayload = HashMap::new();
        vpayload.insert("embedding".to_string(), crate::Value::Float(0.9));
        let fulltext = vec![crate::types::search::SearchHit::new("doc-1", 3.0)
            .with_highlights(vec!["snippet".to_string()])];
        let vector =
            vec![crate::types::search::SearchHit::new("doc-1", 0.9).with_payload(vpayload)];
        // normalize outside? fusion normalizes internally via query crate; core rrf uses ids as-is
        let fused = rrf_fuse(&[fulltext, vector], 60, 10);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].id, "doc-1");
        assert_eq!(
            fused[0].highlights.as_ref().unwrap(),
            &vec!["snippet".to_string()]
        );
        assert!(fused[0].payload.as_ref().unwrap().contains_key("embedding"));
    }
}

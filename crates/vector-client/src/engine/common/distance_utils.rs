//! Score and threshold normalization at the Qdrant client boundary.
//!
//! Crate-wide contract (shared with the local engine in `vector-search`):
//! every [`crate::types::SearchResult`] score is a *similarity* — higher is
//! better — and every `score_threshold` is a *lower bound* on that similarity
//! (`score >= threshold` keeps a candidate).
//!
//! Modern Qdrant (>= 1.10) returns **raw distances** for Euclid collections
//! (lower = nearer) and interprets `score_threshold` as an upper bound on the
//! distance. Cosine/Dot scores already follow the similarity contract on both
//! sides, so only Euclid needs conversion, in both directions:
//!
//! | direction | conversion                              |
//! |-----------|-----------------------------------------|
//! | response  | raw distance `d` → score `1/(1+d)`      |
//! | request   | similarity bound `t` → distance `1/t-1` |
//!
//! These helpers are the single source of truth for the conversions; both the
//! gRPC and the HTTP engine must go through them.

use tracing::warn;

use crate::types::DistanceMetric;

/// Convert a raw Euclid distance into the shared similarity score.
///
/// Distances are non-negative; negative inputs are clamped to 0 (perfect
/// match) instead of producing inverted scores.
pub fn euclid_distance_to_score(distance: f32) -> f32 {
    1.0 / (1.0 + distance.max(0.0))
}

/// Convert a similarity lower-bound threshold into Qdrant's raw-distance
/// upper bound for Euclid collections (`d_max = 1/t - 1`).
///
/// Returns `None` when no constraint should be sent to the server:
/// - NaN or `t <= 0`: every point satisfies `score >= t`;
/// - `t > 1` is clamped down to the maximum achievable score of 1 (exact
///   matches only) before converting.
pub fn euclid_score_threshold_to_distance_threshold(threshold: f32) -> Option<f32> {
    if threshold.is_nan() || threshold <= 0.0 {
        return None;
    }
    Some(1.0 / threshold.min(1.0) - 1.0)
}

/// Normalize a score returned by Qdrant for a collection with the given
/// metric (as resolved by the engine, `None` = unknown) into the shared
/// "higher is better" similarity contract.
pub fn inbound_result_score(metric: Option<DistanceMetric>, qdrant_score: f32) -> f32 {
    match metric {
        Some(DistanceMetric::Euclid) => euclid_distance_to_score(qdrant_score),
        _ => qdrant_score,
    }
}

/// Convert the shared similarity lower-bound threshold into the value to send
/// for a collection with the given metric (as resolved by the engine, `None`
/// = unknown). Returns `None` when nothing should be sent.
pub fn outbound_score_threshold(metric: Option<DistanceMetric>, threshold: f32) -> Option<f32> {
    match metric {
        Some(DistanceMetric::Euclid) => {
            if threshold > 1.0 {
                warn!(
                    threshold,
                    "Euclid similarity threshold exceeds the maximum score of 1.0; clamping to exact-match distance"
                );
            }
            euclid_score_threshold_to_distance_threshold(threshold)
        }
        _ => Some(threshold),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_euclid_distance_to_score() {
        assert!((euclid_distance_to_score(0.0) - 1.0).abs() < f32::EPSILON);
        // d=3 -> 1/4
        assert!((euclid_distance_to_score(3.0) - 0.25).abs() < 1e-6);
        // Monotonically decreasing in the distance.
        assert!(euclid_distance_to_score(1.0) > euclid_distance_to_score(2.0));
        // Negative distances are clamped to a perfect match.
        assert!((euclid_distance_to_score(-5.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_euclid_threshold_round_trip() {
        // t=0.8 -> d_max=0.25; a point at exactly d_max converts back to t.
        let d_max = euclid_score_threshold_to_distance_threshold(0.8).unwrap();
        assert!((d_max - 0.25).abs() < 1e-6);
        assert!((euclid_distance_to_score(d_max) - 0.8).abs() < 1e-6);

        // t=1 -> exact matches only.
        let d_max = euclid_score_threshold_to_distance_threshold(1.0).unwrap();
        assert_eq!(d_max, 0.0);
    }

    #[test]
    fn test_euclid_threshold_vacuous_values_send_nothing() {
        assert!(euclid_score_threshold_to_distance_threshold(0.0).is_none());
        assert!(euclid_score_threshold_to_distance_threshold(-0.5).is_none());
        assert!(euclid_score_threshold_to_distance_threshold(f32::NAN).is_none());
        // t > 1 clamps to exact match rather than being dropped.
        assert_eq!(euclid_score_threshold_to_distance_threshold(2.0), Some(0.0));
    }

    #[test]
    fn test_inbound_result_score_by_metric() {
        // Euclid is normalized, other metrics pass through untouched.
        let converted = inbound_result_score(Some(DistanceMetric::Euclid), 3.0);
        assert!((converted - 0.25).abs() < 1e-6);

        let passthrough_cosine = inbound_result_score(Some(DistanceMetric::Cosine), 3.0);
        assert_eq!(passthrough_cosine, 3.0);

        let passthrough_dot = inbound_result_score(Some(DistanceMetric::Dot), -12.5);
        assert_eq!(passthrough_dot, -12.5);

        // Unknown metric must not guess.
        let unknown = inbound_result_score(None, 3.0);
        assert_eq!(unknown, 3.0);
    }

    #[test]
    fn test_outbound_score_threshold_by_metric() {
        let converted = outbound_score_threshold(Some(DistanceMetric::Euclid), 0.8).unwrap();
        assert!((converted - 0.25).abs() < 1e-6);

        // Vacuous Euclid thresholds are dropped entirely.
        assert!(outbound_score_threshold(Some(DistanceMetric::Euclid), 0.0).is_none());

        // Other metrics keep the similarity threshold as-is.
        let passthrough = outbound_score_threshold(Some(DistanceMetric::Cosine), 0.8).unwrap();
        assert_eq!(passthrough, 0.8);

        let unknown = outbound_score_threshold(None, 0.8).unwrap();
        assert_eq!(unknown, 0.8);
    }
}

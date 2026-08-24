//! Distance kernels and score conversion.
//!
//! The local engine ranks by an internal *distance* (smaller is nearer) and
//! converts it to a *similarity* score for output. That score **is** the
//! crate-wide contract: higher is better, and `score_threshold` is a lower
//! bound on it. Remote Qdrant collections with the Euclid metric return raw
//! distances; the qdrant client normalizes them back to this contract at its
//! boundary (`engine/common/distance_utils.rs`).
//!
//! | metric   | internal distance          | output score        |
//! |----------|----------------------------|---------------------|
//! | Euclid   | `Σ(a-b)²` (squared, no sqrt) | `1/(1+sqrt(d²))`  |
//! | Dot      | `-Σ(a·b)`                  | `Σ(a·b)`            |
//! | Cosine   | `1 - similarity` clamped to `[-1,1]` | `similarity` |
//!
//! The active kernel is selected once per process by [`kernel::selected`]
//! (best available SIMD implementation with a naive fallback); the naive
//! path serves as the correctness baseline and is always exercised by the
//! unit tests.

#[cfg(target_arch = "x86_64")]
pub mod avx2;
pub mod kernel;
pub mod naive;

use crate::types::DistanceMetric;

/// Internal distance between two vectors (smaller = nearer).
#[inline]
pub fn distance(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    kernel::selected().distance(metric, a, b)
}

/// Convert an internal distance to the Qdrant-compatible output score.
pub fn to_score(metric: DistanceMetric, dist: f32) -> f32 {
    match metric {
        DistanceMetric::Euclid | DistanceMetric::Manhattan => 1.0 / (1.0 + dist.sqrt()),
        DistanceMetric::Dot => -dist,
        DistanceMetric::Cosine => 1.0 - dist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distance::avx2 as avx;
    use crate::types::DistanceMetric;

    use rand::Rng;

    fn assert_close(a: f32, b: f32) {
        assert!(
            (a - b).abs() < 1e-5,
            "expected {a} close to {b} (diff {})",
            (a - b).abs()
        );
    }

    #[test]
    fn test_known_values() {
        // L2: a=(0,0) b=(3,4) -> squared distance 25 -> score 1/(1+5) = 1/6
        let d = naive::distance(DistanceMetric::Euclid, &[0.0, 0.0], &[3.0, 4.0]);
        assert_close(d, 25.0);
        assert_close(to_score(DistanceMetric::Euclid, d), 1.0 / 6.0);

        // Dot: a=(1,2,3) b=(4,5,6) -> 32
        let d = naive::distance(DistanceMetric::Dot, &[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
        assert_close(d, -32.0);
        assert_close(to_score(DistanceMetric::Dot, d), 32.0);

        // Cosine: identical unit vectors -> similarity 1 -> distance 0
        let d = naive::distance(DistanceMetric::Cosine, &[1.0, 0.0], &[1.0, 0.0]);
        assert_close(d, 0.0);
        assert_close(to_score(DistanceMetric::Cosine, d), 1.0);

        // Cosine: orthogonal -> similarity 0 -> distance 1
        let d = naive::distance(DistanceMetric::Cosine, &[1.0, 0.0], &[0.0, 1.0]);
        assert_close(d, 1.0);

        // Cosine: opposite -> similarity -1 -> distance 2 (clamped)
        let d = naive::distance(DistanceMetric::Cosine, &[1.0, 0.0], &[-1.0, 0.0]);
        assert_close(d, 2.0);
    }

    #[test]
    fn test_zero_vector_cosine_boundary() {
        // Zero norm: distance 1, score 0 (zero-vector edge).
        let d = naive::distance(DistanceMetric::Cosine, &[0.0, 0.0], &[1.0, 2.0]);
        assert_close(d, 1.0);
        assert_close(to_score(DistanceMetric::Cosine, d), 0.0);
        let d = naive::distance(DistanceMetric::Cosine, &[0.0, 0.0], &[0.0, 0.0]);
        assert_close(d, 1.0);
    }

    #[test]
    fn test_to_score_monotonicity() {
        // Score must be non-increasing as the internal distance grows.
        for metric in [
            DistanceMetric::Euclid,
            DistanceMetric::Dot,
            DistanceMetric::Cosine,
        ] {
            let mut prev = f32::INFINITY;
            for i in 0..100 {
                let dist = (i as f32) * 0.05;
                let score = to_score(metric, dist);
                assert!(
                    score <= prev + 1e-6,
                    "{metric:?}: score {score} grew from {prev} at dist {dist}"
                );
                prev = score;
            }
        }
    }

    #[test]
    fn test_naive_vs_avx2_consistency() {
        #[cfg(target_arch = "x86_64")]
        if !(std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma"))
        {
            eprintln!("avx2 not available, skipping");
            return;
        }

        let mut rng = rand::thread_rng();
        for dim in [1usize, 2, 7, 8, 15, 16, 128, 1025] {
            for _ in 0..20 {
                let a: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect();
                let b: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect();
                for metric in [
                    DistanceMetric::Euclid,
                    DistanceMetric::Dot,
                    DistanceMetric::Cosine,
                ] {
                    let expected = naive::distance(metric, &a, &b);
                    #[cfg(target_arch = "x86_64")]
                    let got = unsafe { avx::distance(metric, &a, &b) };
                    #[cfg(not(target_arch = "x86_64"))]
                    let got = expected;
                    // Tolerance is relative because FMA-based lane
                    // accumulation rounds differently from the scalar loop
                    // (error grows with the magnitude of the sum).
                    let tolerance = 1e-4 * expected.abs().max(1.0);
                    assert!(
                        (expected - got).abs() < tolerance,
                        "{metric:?} dim={dim}: naive {expected} vs avx2 {got}"
                    );
                }
            }
        }

        // Zero-norm boundary: the cosine `denom == 0 -> 1.0` branch must
        // agree on both paths (random vectors above never hit it).
        let zero: Vec<f32> = vec![0.0; 8];
        let ones: Vec<f32> = vec![1.0; 8];
        for (a, b) in [(&zero, &ones), (&ones, &zero), (&zero, &zero)] {
            for metric in [
                DistanceMetric::Euclid,
                DistanceMetric::Dot,
                DistanceMetric::Cosine,
            ] {
                let expected = naive::distance(metric, a, b);
                #[cfg(target_arch = "x86_64")]
                let got = unsafe { avx::distance(metric, a, b) };
                #[cfg(not(target_arch = "x86_64"))]
                let got = expected;
                assert_eq!(got, expected, "{metric:?} zero-norm");
            }
        }
    }

    #[test]
    fn test_selected_kernel() {
        use crate::distance::kernel::Kernel;

        let expected = if cfg!(target_arch = "x86_64")
            && std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
        {
            Kernel::Avx2
        } else {
            Kernel::Naive
        };
        assert_eq!(kernel::selected(), expected);

        // Pinning the naive kernel must produce identical results through
        // the dispatch path.
        kernel::force_for_test(Kernel::Naive);
        assert_eq!(kernel::selected(), Kernel::Naive);
        let a = [0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let b = [0.8f32, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1];
        for metric in [
            DistanceMetric::Euclid,
            DistanceMetric::Dot,
            DistanceMetric::Cosine,
        ] {
            assert_eq!(distance(metric, &a, &b), naive::distance(metric, &a, &b));
        }
    }
}

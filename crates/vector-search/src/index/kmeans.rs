//! Sampled k-means training for the IVFFlat index.
//!
//! Pure functions over borrowed sample slices; sampling from mmap-backed
//! storage happens in the caller (`CollectionStore::build_index`). Training is
//! fully deterministic: a fixed-seed xorshift generator drives both the
//! k-means++ initialization and empty-cluster reseeding, so repeated builds
//! over the same data produce identical centroids.

use rayon::prelude::*;

use crate::error::{Result, VectorSearchError};
use crate::types::DistanceMetric;

/// Mean centroid movement below which training stops early.
const CONVERGENCE_TOL: f32 = 1e-3;

/// Deterministic xorshift64* generator (no external dependency).
pub(crate) struct XorShift(u64);

impl XorShift {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let x = self.0;
        self.0 ^= x << 13;
        self.0 ^= x >> 7;
        self.0 ^= x << 17;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform float in `[0, 1)`.
    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Uniform integer in `[0, bound)` (bound > 0).
    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

#[derive(Debug, Clone)]
pub(crate) struct KmeansOptions {
    pub k: u32,
    pub dim: usize,
    pub max_iter: u32,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct KmeansResult {
    /// `k x dim` cluster centroids.
    pub centroids: Vec<Vec<f32>>,
}

/// Train k-means on `sample`. The effective `k` is clamped down to the sample
/// size; an empty sample is an error.
pub(crate) fn train(
    metric: DistanceMetric,
    sample: &[&[f32]],
    opts: &KmeansOptions,
) -> Result<KmeansResult> {
    if sample.is_empty() {
        return Err(VectorSearchError::Internal(
            "k-means training requires a non-empty sample".to_string(),
        ));
    }
    let k = (opts.k.max(1) as usize).min(sample.len());
    let mut rng = XorShift::new(opts.seed);

    let mut centroids = kmeans_pp_init(metric, sample, k, &mut rng);
    let mut labels = vec![0u32; sample.len()];

    for _ in 0..opts.max_iter {
        // Assignment step (parallel over the sample).
        labels = sample
            .par_iter()
            .map(|v| nearest_centroid(metric, v, &centroids) as u32)
            .collect();

        // Update step: accumulate member sums, then average.
        let mut sums = vec![vec![0.0f32; opts.dim]; k];
        let mut counts = vec![0u64; k];
        for (v, &l) in sample.iter().zip(labels.iter()) {
            counts[l as usize] += 1;
            let sum = &mut sums[l as usize];
            for (s, x) in sum.iter_mut().zip(v.iter()) {
                *s += x;
            }
        }

        let mut shift = 0.0f32;
        for c in 0..k {
            if counts[c] == 0 {
                // Empty cluster: reseed with the point farthest from its
                // centroid (deterministic tie-break by lowest sample index).
                let far = farthest_point(metric, sample, &centroids);
                shift += centroid_shift(&centroids[c], sample[far]);
                centroids[c] = sample[far].to_vec();
                continue;
            }
            let mut next = std::mem::take(&mut sums[c]);
            for x in &mut next {
                *x /= counts[c] as f32;
            }
            shift += centroid_shift(&centroids[c], &next);
            centroids[c] = next;
        }

        if shift / (k as f32) < CONVERGENCE_TOL {
            break;
        }
    }

    Ok(KmeansResult { centroids })
}

/// Nearest centroid index for one vector.
pub(crate) fn nearest_centroid(metric: DistanceMetric, v: &[f32], centroids: &[Vec<f32>]) -> usize {
    let mut best = 0usize;
    let mut best_dist = f32::INFINITY;
    for (i, c) in centroids.iter().enumerate() {
        let d = crate::distance::distance(metric, v, c);
        if d < best_dist {
            best_dist = d;
            best = i;
        }
    }
    best
}

/// k-means++ seeding: first centroid uniform-random, each following centroid
/// sampled with probability proportional to squared distance (approximated by
/// the metric's internal distance) to the closest existing centroid.
fn kmeans_pp_init(
    metric: DistanceMetric,
    sample: &[&[f32]],
    k: usize,
    rng: &mut XorShift,
) -> Vec<Vec<f32>> {
    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(k);
    centroids.push(sample[rng.below(sample.len())].to_vec());

    let mut min_dists: Vec<f32> = sample
        .iter()
        .map(|v| crate::distance::distance(metric, v, &centroids[0]))
        .collect();

    while centroids.len() < k {
        let total: f32 = min_dists.iter().sum();
        let next = if total <= 0.0 {
            // Degenerate (all points identical / zero mass): uniform choice.
            sample[rng.below(sample.len())].to_vec()
        } else {
            let mut target = rng.next_f32() * total;
            let mut idx = sample.len() - 1;
            for (i, &d) in min_dists.iter().enumerate() {
                target -= d;
                if target <= 0.0 {
                    idx = i;
                    break;
                }
            }
            sample[idx].to_vec()
        };
        for (v, md) in sample.iter().zip(min_dists.iter_mut()) {
            let d = crate::distance::distance(metric, v, &next);
            if d < *md {
                *md = d;
            }
        }
        centroids.push(next);
    }
    centroids
}

fn farthest_point(metric: DistanceMetric, sample: &[&[f32]], centroids: &[Vec<f32>]) -> usize {
    let mut best = 0usize;
    let mut best_dist = f32::NEG_INFINITY;
    for (i, v) in sample.iter().enumerate() {
        let c = &centroids[nearest_centroid(metric, v, centroids)];
        let d = crate::distance::distance(metric, v, c);
        if d > best_dist {
            best_dist = d;
            best = i;
        }
    }
    best
}

fn centroid_shift(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const DIM: usize = 8;

    /// Three well-separated gaussian-ish blobs.
    fn clustered_sample() -> Vec<Vec<f32>> {
        let centers: [[f32; DIM]; 3] = [[0.0; DIM], [10.0; DIM], [20.0; DIM]];
        let mut rng = StdRng::seed_from_u64(42);
        let mut out = Vec::with_capacity(300);
        for center in &centers {
            for _ in 0..100 {
                let mut v = [0.0f32; DIM];
                for x in &mut v {
                    *x = center[0] + rng.gen_range(-0.5..0.5);
                }
                out.push(v.to_vec());
            }
        }
        out
    }

    #[test]
    fn test_train_separates_clusters() {
        let data = clustered_sample();
        let refs: Vec<&[f32]> = data.iter().map(|v| v.as_slice()).collect();
        let result = train(
            DistanceMetric::Euclid,
            &refs,
            &KmeansOptions {
                k: 3,
                dim: DIM,
                max_iter: 10,
                seed: 7,
            },
        )
        .unwrap();

        assert_eq!(result.centroids.len(), 3);
        // Every point must land in the list of a centroid near one of the
        // three blob centers; intra-cluster spread stays tiny compared to the
        // inter-center distance of 10.
        for v in &refs {
            let c =
                &result.centroids[nearest_centroid(DistanceMetric::Euclid, v, &result.centroids)];
            let dist = crate::distance::distance(DistanceMetric::Euclid, v, c).sqrt();
            assert!(dist < 2.0, "point {v:?} landed {dist} away from {c:?}");
        }
    }

    #[test]
    fn test_train_is_deterministic() {
        let data = clustered_sample();
        let refs: Vec<&[f32]> = data.iter().map(|v| v.as_slice()).collect();
        let opts = KmeansOptions {
            k: 4,
            dim: DIM,
            max_iter: 10,
            seed: 99,
        };
        let a = train(DistanceMetric::Cosine, &refs, &opts).unwrap();
        let b = train(DistanceMetric::Cosine, &refs, &opts).unwrap();
        assert_eq!(a.centroids, b.centroids);
    }

    #[test]
    fn test_train_clamps_k_to_sample() {
        let data = [vec![1.0f32; DIM], vec![2.0f32; DIM]];
        let refs: Vec<&[f32]> = data.iter().map(|v| v.as_slice()).collect();
        let result = train(
            DistanceMetric::Euclid,
            &refs,
            &KmeansOptions {
                k: 16,
                dim: DIM,
                max_iter: 5,
                seed: 1,
            },
        )
        .unwrap();
        assert_eq!(result.centroids.len(), 2);
    }

    #[test]
    fn test_empty_sample_errors() {
        let err = train(
            DistanceMetric::Euclid,
            &[],
            &KmeansOptions {
                k: 2,
                dim: DIM,
                max_iter: 5,
                seed: 1,
            },
        )
        .unwrap_err();
        assert!(matches!(err, VectorSearchError::Internal(_)));
    }

    #[test]
    fn test_xorshift_bounds() {
        let mut rng = XorShift::new(12345);
        for _ in 0..10_000 {
            let v = rng.below(7);
            assert!(v < 7);
            let f = rng.next_f32();
            assert!((0.0..1.0).contains(&f));
        }
    }
}

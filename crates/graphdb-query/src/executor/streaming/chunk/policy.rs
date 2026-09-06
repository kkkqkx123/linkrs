//! ColumnarPolicy Module
//!
//! Cross-query adaptive policy for the typed columnar chunk layout.
//!
//! The policy learns from per-query [`crate::executor::streaming::runtime::ColumnarStats`]:
//! each query's columnar hit/miss counters are merged into the shared policy
//! at query completion, and the policy decides whether the typed columnar
//! path is worth building for subsequent queries.

use std::sync::atomic::{AtomicU64, Ordering};

/// Adaptive gate for the typed columnar chunk layout (cross-query shared).
///
/// Uses accumulated columnar hit/miss counts with a hit-rate threshold and a
/// minimum sample count.  Below `min_samples` the policy defaults to
/// columnar (matching the historical default-on behavior, so there is no
/// performance regression at startup).
///
/// # Thread Safety
///
/// All counters are `Relaxed` atomics; `should_use_columnar` never blocks.
/// The policy values are only mutated between queries (when merging per-query
/// snapshots), so reads within a single query are stable and a query never
/// flips layout mid-flight.
#[derive(Debug)]
pub struct ColumnarPolicy {
    /// Cumulative columnar fast-path hits across queries.
    hits: AtomicU64,
    /// Cumulative row-wise fallbacks across queries.
    misses: AtomicU64,
    /// Hit-rate threshold: use the columnar path when hits/total >= this.
    threshold: f64,
    /// Minimum total samples before the threshold decision applies.
    min_samples: u64,
}

impl ColumnarPolicy {
    /// Create a policy with explicit threshold and minimum sample count.
    pub fn new(threshold: f64, min_samples: u64) -> Self {
        Self {
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            threshold: threshold.clamp(0.0, 1.0),
            min_samples: min_samples.max(1),
        }
    }

    /// Decide whether the typed columnar path should be used.
    ///
    /// Returns `true` while the sample count is below `min_samples`
    /// (default-on), and thereafter when the cumulative hit rate stays at or
    /// above the threshold.
    pub fn should_use_columnar(&self) -> bool {
        let total = self.hits.load(Ordering::Relaxed) + self.misses.load(Ordering::Relaxed);
        if total < self.min_samples {
            return true;
        }
        self.hits.load(Ordering::Relaxed) as f64 / total as f64 >= self.threshold
    }

    /// Record a single hit / miss decision (used by tests and single-thread
    /// simulations).
    pub fn record(&self, hit: bool) {
        if hit {
            self.record_hit();
        } else {
            self.record_miss();
        }
    }

    /// Record one columnar fast-path hit.
    pub fn record_hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one row-wise fallback.
    pub fn record_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Merge a per-query columnar snapshot into the shared counters.
    ///
    /// Called once per completed query (materialized and streaming paths);
    /// `hits` / `misses` are the deltas of that query's
    /// [`crate::executor::streaming::runtime::ColumnarStats`].
    pub fn merge(&self, hits: u64, misses: u64) {
        if hits > 0 {
            self.hits.fetch_add(hits, Ordering::Relaxed);
        }
        if misses > 0 {
            self.misses.fetch_add(misses, Ordering::Relaxed);
        }
    }

    /// Point-in-time snapshot of `(hits, misses)`.
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        )
    }

    /// Cumulative hit rate over all recorded samples (1.0 when empty).
    pub fn hit_rate(&self) -> f64 {
        let (hits, misses) = self.snapshot();
        let total = hits + misses;
        if total == 0 {
            1.0
        } else {
            hits as f64 / total as f64
        }
    }

    /// The configured hit-rate threshold.
    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    /// The configured minimum sample count.
    pub fn min_samples(&self) -> u64 {
        self.min_samples
    }
}

impl Default for ColumnarPolicy {
    fn default() -> Self {
        Self::new(0.8, 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_on_below_min_samples() {
        let policy = ColumnarPolicy::new(0.8, 100);
        // No samples at all: default to columnar.
        assert!(policy.should_use_columnar());
        // Below the sample floor the decision stays columnar even when the
        // early hit rate is poor.
        for _ in 0..50 {
            policy.record_miss();
        }
        assert!(policy.should_use_columnar());
    }

    #[test]
    fn test_flips_to_row_after_enough_fallbacks() {
        let policy = ColumnarPolicy::new(0.8, 100);
        // 150 samples with 90% fallbacks: hit rate 0.1 < 0.8 -> row path.
        for _ in 0..15 {
            policy.record_hit();
        }
        for _ in 0..135 {
            policy.record_miss();
        }
        assert!(!policy.should_use_columnar());
        assert_eq!(policy.snapshot(), (15, 135));
    }

    #[test]
    fn test_recovers_to_columnar_after_hits() {
        let policy = ColumnarPolicy::new(0.8, 100);
        for _ in 0..100 {
            policy.record_miss();
        }
        assert!(!policy.should_use_columnar());

        // Sustained hits flip the decision back.
        for _ in 0..400 {
            policy.record_hit();
        }
        assert!(policy.should_use_columnar());
    }

    #[test]
    fn test_merge_accumulates_deltas() {
        let policy = ColumnarPolicy::new(0.8, 100);
        policy.merge(60, 40);
        policy.merge(60, 40);
        assert_eq!(policy.snapshot(), (120, 80));
        // 60% hit rate: below the 0.8 threshold, so the row path wins.
        assert!(policy.hit_rate() > 0.5);
        assert!(!policy.should_use_columnar());

        // Sustained hits from later queries bring the rate back up.
        for _ in 0..8 {
            policy.merge(60, 0);
        }
        assert!(policy.should_use_columnar());
    }

    #[test]
    fn test_threshold_boundary() {
        // Exactly at the threshold: columnar stays selected.
        let policy = ColumnarPolicy::new(0.5, 100);
        for _ in 0..50 {
            policy.record_hit();
            policy.record_miss();
        }
        assert!(policy.should_use_columnar());
    }
}

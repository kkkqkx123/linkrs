//! Decision-level execution feedback
//!
//! Closes the loop on the Apply-vs-Join (decorrelation) decision: records
//! which execution path was taken and the measured rows / time of each path
//! per space, and produces an advice for the cost-based unnesting decision.

use std::collections::HashMap;

use parking_lot::RwLock;

/// Minimum total runs before the empirical advice is allowed to vote.
const MIN_DECISION_RUNS: u64 = 8;

/// Advice derived from measured Apply / SemiJoin executions.
#[derive(Debug, Clone, Default)]
pub struct DecorrelationAdvice {
    /// Measured evidence prefers unnesting (SemiJoin was faster).
    pub prefer_unnest: bool,
    /// Measured evidence prefers keeping the nested-loop Apply.
    pub prefer_keep: bool,
    /// Confidence of the empirical vote (0..1, sigmoid in the run count).
    pub confidence: f64,
    /// Measured EWMA cost of one nested-loop subquery execution per left
    /// row (time per row in microseconds), if any.
    pub apply_cost_per_row: Option<f64>,
}

/// Per-space accumulated measurement of both decorrelation paths.
#[derive(Debug, Clone, Default)]
struct SpaceDecisionStats {
    apply_runs: u64,
    apply_rows: f64,
    apply_time_us: f64,
    join_runs: u64,
    join_rows: f64,
    join_time_us: f64,
}

impl SpaceDecisionStats {
    fn total_runs(&self) -> u64 {
        self.apply_runs + self.join_runs
    }

    /// Average nested-loop cost per output row (us).
    fn apply_cost_per_row(&self) -> Option<f64> {
        if self.apply_runs > 0 && self.apply_rows > 0.0 {
            Some(self.apply_time_us / self.apply_rows)
        } else {
            None
        }
    }

    /// Average hash-path cost per output row (us).
    fn join_cost_per_row(&self) -> Option<f64> {
        if self.join_runs > 0 && self.join_rows > 0.0 {
            Some(self.join_time_us / self.join_rows)
        } else {
            None
        }
    }
}

/// Shared store of decorrelation decision feedback, keyed by space.
#[derive(Debug, Default)]
pub struct DecisionFeedbackStore {
    /// Mapping from `"{space}:decorrelation"` to accumulated measurements.
    stats: RwLock<HashMap<String, SpaceDecisionStats>>,
}

impl DecisionFeedbackStore {
    /// Create a new decision feedback store.
    pub fn new() -> Self {
        Self::default()
    }

    fn key(space: &str) -> String {
        format!("{space}:decorrelation")
    }

    /// Record one execution of the kept nested-loop Apply path.
    pub fn record_apply_run(&self, space: &str, rows: u64, time_us: u64) {
        let mut stats = self.stats.write();
        let entry = stats.entry(Self::key(space)).or_default();
        entry.apply_runs += 1;
        entry.apply_rows += rows as f64;
        entry.apply_time_us += time_us as f64;
    }

    /// Record one execution of the unnested SemiJoin path.
    pub fn record_join_run(&self, space: &str, rows: u64, time_us: u64) {
        let mut stats = self.stats.write();
        let entry = stats.entry(Self::key(space)).or_default();
        entry.join_runs += 1;
        entry.join_rows += rows as f64;
        entry.join_time_us += time_us as f64;
    }

    /// The empirical advice for the decorrelation decision in `space`.
    pub fn advice(&self, space: &str) -> DecorrelationAdvice {
        let stats = self.stats.read();
        let Some(entry) = stats.get(&Self::key(space)) else {
            return DecorrelationAdvice::default();
        };
        let runs = entry.total_runs();
        let confidence = {
            let x = runs as f64 * 0.1;
            1.0 / (1.0 + (-x).exp())
        };
        let mut advice = DecorrelationAdvice {
            confidence,
            apply_cost_per_row: entry.apply_cost_per_row(),
            ..Default::default()
        };
        if runs >= MIN_DECISION_RUNS {
            match (entry.apply_cost_per_row(), entry.join_cost_per_row()) {
                (Some(apply_cost), Some(join_cost)) => {
                    advice.prefer_unnest = join_cost < apply_cost;
                    advice.prefer_keep = apply_cost <= join_cost;
                }
                (Some(_), None) => {
                    // Only the kept path has been observed: keep is the
                    // incumbent and there is no counter-evidence.
                    advice.prefer_keep = true;
                }
                _ => {}
            }
        }
        advice
    }

    /// Drop all measurements for a space (`None` clears everything).
    pub fn invalidate_space(&self, space: Option<&str>) -> usize {
        let mut stats = self.stats.write();
        match space {
            Some(space) => stats.remove(&Self::key(space)).is_some() as usize,
            None => {
                let removed = stats.len();
                stats.clear();
                removed
            }
        }
    }
}

impl Clone for DecisionFeedbackStore {
    fn clone(&self) -> Self {
        Self {
            stats: RwLock::new(self.stats.read().clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_advice_neutral_without_evidence() {
        let store = DecisionFeedbackStore::new();
        let advice = store.advice("s");
        assert!(!advice.prefer_unnest && !advice.prefer_keep);
        assert!(advice.apply_cost_per_row.is_none());
    }

    #[test]
    fn test_advice_prefers_faster_path() {
        let store = DecisionFeedbackStore::new();
        // Apply path: 10 runs, 1000 rows total, 10000us total -> 10us/row.
        for _ in 0..10 {
            store.record_apply_run("s", 100, 10_000);
        }
        // Join path: 10 runs, 1000 rows total, 1000us total -> 1us/row.
        for _ in 0..10 {
            store.record_join_run("s", 100, 1_000);
        }
        let advice = store.advice("s");
        assert!(advice.prefer_unnest);
        assert!(!advice.prefer_keep);
        assert!(advice.confidence > 0.7);
        assert!(advice.apply_cost_per_row.is_some());
    }

    #[test]
    fn test_advice_prefers_keep_when_apply_is_faster() {
        let store = DecisionFeedbackStore::new();
        for _ in 0..10 {
            store.record_apply_run("s", 100, 1_000);
            store.record_join_run("s", 100, 10_000);
        }
        let advice = store.advice("s");
        assert!(!advice.prefer_unnest);
        assert!(advice.prefer_keep);
    }

    #[test]
    fn test_advice_requires_minimum_runs() {
        let store = DecisionFeedbackStore::new();
        // 3 runs on each side: below the voting threshold.
        for _ in 0..3 {
            store.record_apply_run("s", 100, 100_000);
            store.record_join_run("s", 100, 1_000);
        }
        let advice = store.advice("s");
        assert!(!advice.prefer_unnest && !advice.prefer_keep);
    }

    #[test]
    fn test_invalidate_space() {
        let store = DecisionFeedbackStore::new();
        store.record_apply_run("s", 10, 100);
        assert_eq!(store.invalidate_space(Some("s")), 1);
        assert!(!store.advice("s").prefer_keep && !store.advice("s").prefer_unnest);
    }
}

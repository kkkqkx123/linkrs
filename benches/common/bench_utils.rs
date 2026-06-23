// benches/common/bench_utils.rs
//! Benchmark utility functions

use criterion::{Criterion, BenchmarkGroup};
use std::time::Duration;

/// Create a standard benchmark group configuration
pub fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);

    group.measurement_time(Duration::from_secs(10));
    group.sample_size(100);
    group.warm_up_time(Duration::from_secs(1));

    group
}

/// Create a short benchmark group for quick tests
pub fn create_short_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);

    group.measurement_time(Duration::from_secs(3));
    group.sample_size(50);
    group.warm_up_time(Duration::from_millis(500));

    group
}

/// Performance comparison helper
#[derive(Debug)]
pub struct PerformanceComparison {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
}

impl PerformanceComparison {
    pub fn new(metric: &str, baseline: f64, current: f64) -> Self {
        Self {
            metric: metric.to_string(),
            baseline,
            current,
        }
    }

    /// Calculate improvement percentage (positive = improvement)
    pub fn improvement_percent(&self) -> f64 {
        ((self.baseline - self.current) / self.baseline) * 100.0
    }

    /// Check if meets expectation threshold
    pub fn meets_expectation(&self, threshold_percent: f64) -> bool {
        self.improvement_percent() >= threshold_percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_performance_comparison() {
        let comp = PerformanceComparison::new("latency", 1.0, 0.8);
        assert!(comp.improvement_percent() > 19.0 && comp.improvement_percent() < 21.0);
    }
}

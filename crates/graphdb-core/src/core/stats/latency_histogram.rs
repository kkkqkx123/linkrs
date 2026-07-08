//! Latency histogram with percentile calculations
//!
//! Provides P50, P95, P99 percentile calculations for query latency analysis.

use std::collections::VecDeque;
use std::time::Duration;

/// Latency histogram for calculating percentiles
///
/// Stores latency samples in microseconds and provides percentile calculations.
/// Used for analyzing query latency distribution including tail latencies.
#[derive(Debug, Clone)]
pub struct LatencyHistogram {
    latencies: VecDeque<u64>, // in microseconds
    max_samples: usize,
}

impl LatencyHistogram {
    /// Create a new latency histogram with specified max samples
    pub fn new(max_samples: usize) -> Self {
        Self {
            latencies: VecDeque::with_capacity(max_samples),
            max_samples,
        }
    }

    /// Record a latency sample from Duration
    pub fn record(&mut self, duration: Duration) {
        let micros = duration.as_micros() as u64;
        self.record_micros(micros);
    }

    /// Record a latency sample in microseconds
    pub fn record_micros(&mut self, micros: u64) {
        if self.latencies.len() >= self.max_samples {
            self.latencies.pop_front();
        }
        self.latencies.push_back(micros);
    }

    /// Calculate P50 (median) in microseconds
    pub fn p50(&self) -> u64 {
        self.percentile(50.0)
    }

    /// Calculate P95 in microseconds
    pub fn p95(&self) -> u64 {
        self.percentile(95.0)
    }

    /// Calculate P99 in microseconds
    pub fn p99(&self) -> u64 {
        self.percentile(99.0)
    }

    /// Calculate arbitrary percentile
    ///
    /// # Arguments
    /// * `p` - Percentile value (0.0 - 100.0)
    pub fn percentile(&self, p: f64) -> u64 {
        if self.latencies.is_empty() {
            return 0;
        }

        let mut sorted: Vec<u64> = self.latencies.iter().copied().collect();
        sorted.sort_unstable();

        let index = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    }

    /// Calculate average latency in microseconds
    pub fn avg(&self) -> u64 {
        if self.latencies.is_empty() {
            return 0;
        }
        self.latencies.iter().sum::<u64>() / self.latencies.len() as u64
    }

    /// Get minimum latency in microseconds
    pub fn min(&self) -> u64 {
        self.latencies.iter().copied().min().unwrap_or(0)
    }

    /// Get maximum latency in microseconds
    pub fn max(&self) -> u64 {
        self.latencies.iter().copied().max().unwrap_or(0)
    }

    /// Get total sample count
    pub fn count(&self) -> usize {
        self.latencies.len()
    }

    /// Clear all samples
    pub fn clear(&mut self) {
        self.latencies.clear();
    }

    /// Get latency statistics as a formatted string
    pub fn report(&self) -> String {
        format!(
            "Latency Stats ({} samples): avg={}us, min={}us, max={}us, P50={}us, P95={}us, P99={}us",
            self.count(),
            self.avg(),
            self.min(),
            self.max(),
            self.p50(),
            self.p95(),
            self.p99()
        )
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new(10000) // Default 10k samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_histogram_basic() {
        let mut histogram = LatencyHistogram::new(100);

        // Record some latencies
        histogram.record_micros(100);
        histogram.record_micros(200);
        histogram.record_micros(300);

        assert_eq!(histogram.count(), 3);
        assert_eq!(histogram.avg(), 200);
        assert_eq!(histogram.min(), 100);
        assert_eq!(histogram.max(), 300);
    }

    #[test]
    fn test_latency_histogram_percentiles() {
        let mut histogram = LatencyHistogram::new(100);

        // Record 100 samples from 1 to 100
        for i in 1..=100 {
            histogram.record_micros(i);
        }

        // P50 should be around 50
        let p50 = histogram.p50();
        assert!(
            (49..=51).contains(&p50),
            "P50 should be around 50, got {}",
            p50
        );

        // P95 should be around 95
        let p95 = histogram.p95();
        assert!(
            (94..=96).contains(&p95),
            "P95 should be around 95, got {}",
            p95
        );

        // P99 should be around 99
        let p99 = histogram.p99();
        assert!(
            (98..=100).contains(&p99),
            "P99 should be around 99, got {}",
            p99
        );
    }

    #[test]
    fn test_latency_histogram_with_duration() {
        let mut histogram = LatencyHistogram::new(100);

        histogram.record(Duration::from_micros(100));
        histogram.record(Duration::from_millis(1)); // 1000 microseconds

        assert_eq!(histogram.count(), 2);
        assert_eq!(histogram.min(), 100);
        assert_eq!(histogram.max(), 1000);
    }

    #[test]
    fn test_latency_histogram_max_samples() {
        let mut histogram = LatencyHistogram::new(5);

        // Record more than max samples
        for i in 1..=10 {
            histogram.record_micros(i);
        }

        // Should only keep last 5 samples
        assert_eq!(histogram.count(), 5);
        assert_eq!(histogram.min(), 6);
        assert_eq!(histogram.max(), 10);
    }

    #[test]
    fn test_latency_histogram_empty() {
        let histogram: LatencyHistogram = LatencyHistogram::new(100);

        assert_eq!(histogram.count(), 0);
        assert_eq!(histogram.avg(), 0);
        assert_eq!(histogram.p50(), 0);
        assert_eq!(histogram.p95(), 0);
        assert_eq!(histogram.p99(), 0);
    }

    #[test]
    fn test_latency_histogram_clear() {
        let mut histogram = LatencyHistogram::new(100);

        histogram.record_micros(100);
        histogram.record_micros(200);

        assert_eq!(histogram.count(), 2);

        histogram.clear();

        assert_eq!(histogram.count(), 0);
        assert_eq!(histogram.avg(), 0);
    }

    #[test]
    fn test_latency_histogram_report() {
        let mut histogram = LatencyHistogram::new(100);

        histogram.record_micros(100);
        histogram.record_micros(200);
        histogram.record_micros(300);

        let report = histogram.report();
        assert!(report.contains("3 samples"));
        assert!(report.contains("avg=200us"));
        assert!(report.contains("P50="));
    }
}

// benches/analyzer/metrics.rs
//! Performance metrics data structures and analysis

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Core performance analysis metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisMetrics {
    /// Query planning time in milliseconds
    pub planning_time_ms: f64,

    /// Query execution time in milliseconds
    pub execution_time_ms: f64,

    /// Startup latency (time to first row) in milliseconds
    pub startup_time_ms: f64,

    /// Total rows processed
    pub total_rows: usize,

    /// Peak memory usage in bytes
    pub peak_memory_bytes: usize,

    /// Rows per second throughput
    pub throughput: f64,

    /// Cache hit rate (0.0 to 1.0)
    pub cache_hit_rate: f64,

    /// Number of plan nodes
    pub plan_complexity: usize,

    /// Per-node statistics
    pub node_stats: Vec<NodeMetrics>,

    /// Timestamp for tracking
    pub timestamp: String,
}

/// Per-node performance metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMetrics {
    pub node_id: i64,
    pub node_name: String,
    pub output_rows: usize,
    pub execution_time_ms: f64,
    pub memory_used_bytes: usize,
    pub throughput_rows_per_sec: f64,
}

/// Comparison result between baseline and current metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonResult {
    pub baseline: AnalysisMetrics,
    pub current: AnalysisMetrics,
    pub deviations: HashMap<String, f64>,
    pub has_regression: bool,
    pub regressions: Vec<RegressionInfo>,
}

/// Regression information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionInfo {
    pub metric_name: String,
    pub baseline_value: f64,
    pub current_value: f64,
    pub deviation_percent: f64,
    pub severity: RegressionSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegressionSeverity {
    Low,
    Medium,
    High,
    Critical,
}

impl AnalysisMetrics {
    /// Calculate performance score (0-100)
    /// Higher is better
    pub fn calculate_score(&self) -> f64 {
        let mut score = 100.0;

        // Deduct for planning time
        if self.planning_time_ms > 100.0 {
            score -= (self.planning_time_ms / 10.0).min(20.0);
        }

        // Deduct for execution time
        if self.execution_time_ms > 1000.0 {
            score -= (self.execution_time_ms / 100.0).min(20.0);
        }

        // Deduct for startup latency
        if self.startup_time_ms > 50.0 {
            score -= (self.startup_time_ms / 10.0).min(10.0);
        }

        // Deduct for memory usage
        if self.peak_memory_bytes > 100 * 1024 * 1024 {
            let memory_mb = self.peak_memory_bytes as f64 / (1024.0 * 1024.0);
            score -= (memory_mb / 10.0).min(15.0);
        }

        // Deduct for low throughput
        if self.throughput < 1000.0 && self.total_rows > 0 {
            score -= ((1000.0 - self.throughput) / 100.0).min(15.0);
        }

        // Bonus for high cache hit rate
        if self.cache_hit_rate > 0.8 {
            score += 5.0;
        }

        score.max(0.0).min(100.0)
    }

    /// Generate human-readable summary
    pub fn summary(&self) -> String {
        format!(
            "Planning: {:.2}ms | Exec: {:.2}ms | Startup: {:.2}ms | Rows: {} | Memory: {:.2}MB | Throughput: {:.0} rows/sec | Cache Hit: {:.1}% | Score: {:.1}",
            self.planning_time_ms,
            self.execution_time_ms,
            self.startup_time_ms,
            self.total_rows,
            self.peak_memory_bytes as f64 / (1024.0 * 1024.0),
            self.throughput,
            self.cache_hit_rate * 100.0,
            self.calculate_score()
        )
    }

    /// Generate detailed performance report
    pub fn detailed_report(&self) -> String {
        let mut report = String::new();

        report.push_str("=== Performance Analysis Report ===\n\n");

        report.push_str("Planning Phase:\n");
        report.push_str(&format!("  Planning Time: {:.2}ms\n", self.planning_time_ms));
        report.push_str(&format!("  Plan Nodes: {}\n\n", self.plan_complexity));

        report.push_str("Execution Phase:\n");
        report.push_str(&format!("  Execution Time: {:.2}ms\n", self.execution_time_ms));
        report.push_str(&format!("  Startup Time: {:.2}ms\n", self.startup_time_ms));
        report.push_str(&format!("  Total Rows: {}\n", self.total_rows));
        report.push_str(&format!(
            "  Peak Memory: {:.2}MB\n",
            self.peak_memory_bytes as f64 / (1024.0 * 1024.0)
        ));
        report.push_str(&format!("  Throughput: {:.0} rows/sec\n\n", self.throughput));

        report.push_str("Cache Analysis:\n");
        report.push_str(&format!(
            "  Cache Hit Rate: {:.2}%\n\n",
            self.cache_hit_rate * 100.0
        ));

        report.push_str("Node Analysis:\n");
        for node in &self.node_stats {
            report.push_str(&format!(
                "  Node {} ({}): {} rows, {:.2}ms, {:.0} rows/sec, {:.2}MB\n",
                node.node_id,
                node.node_name,
                node.output_rows,
                node.execution_time_ms,
                node.throughput_rows_per_sec,
                node.memory_used_bytes as f64 / (1024.0 * 1024.0)
            ));
        }

        report.push_str("\n");
        report.push_str(&format!("Overall Score: {:.1}/100\n", self.calculate_score()));

        report
    }
}

impl ComparisonResult {
    /// Create a new comparison result
    pub fn new(baseline: AnalysisMetrics, current: AnalysisMetrics) -> Self {
        let mut deviations = HashMap::new();
        let mut regressions = vec![];

        // Compare planning time
        let planning_deviation = if baseline.planning_time_ms > 0.0 {
            ((current.planning_time_ms - baseline.planning_time_ms)
                / baseline.planning_time_ms)
                * 100.0
        } else {
            0.0
        };
        deviations.insert("planning_time".to_string(), planning_deviation);

        if planning_deviation > 10.0 {
            let severity = if planning_deviation > 50.0 {
                RegressionSeverity::Critical
            } else if planning_deviation > 30.0 {
                RegressionSeverity::High
            } else {
                RegressionSeverity::Medium
            };

            regressions.push(RegressionInfo {
                metric_name: "Planning Time".to_string(),
                baseline_value: baseline.planning_time_ms,
                current_value: current.planning_time_ms,
                deviation_percent: planning_deviation,
                severity,
            });
        }

        // Compare execution time
        let execution_deviation = if baseline.execution_time_ms > 0.0 {
            ((current.execution_time_ms - baseline.execution_time_ms)
                / baseline.execution_time_ms)
                * 100.0
        } else {
            0.0
        };
        deviations.insert("execution_time".to_string(), execution_deviation);

        if execution_deviation > 10.0 {
            let severity = if execution_deviation > 50.0 {
                RegressionSeverity::Critical
            } else if execution_deviation > 30.0 {
                RegressionSeverity::High
            } else {
                RegressionSeverity::Medium
            };

            regressions.push(RegressionInfo {
                metric_name: "Execution Time".to_string(),
                baseline_value: baseline.execution_time_ms,
                current_value: current.execution_time_ms,
                deviation_percent: execution_deviation,
                severity,
            });
        }

        // Compare memory usage
        let memory_deviation = if baseline.peak_memory_bytes > 0 {
            ((current.peak_memory_bytes as i64 - baseline.peak_memory_bytes as i64)
                as f64
                / baseline.peak_memory_bytes as f64)
                * 100.0
        } else {
            0.0
        };
        deviations.insert("memory".to_string(), memory_deviation);

        if memory_deviation > 20.0 {
            let severity = if memory_deviation > 100.0 {
                RegressionSeverity::Critical
            } else if memory_deviation > 50.0 {
                RegressionSeverity::High
            } else {
                RegressionSeverity::Medium
            };

            regressions.push(RegressionInfo {
                metric_name: "Peak Memory".to_string(),
                baseline_value: baseline.peak_memory_bytes as f64,
                current_value: current.peak_memory_bytes as f64,
                deviation_percent: memory_deviation,
                severity,
            });
        }

        let has_regression = !regressions.is_empty();

        Self {
            baseline,
            current,
            deviations,
            has_regression,
            regressions,
        }
    }

    /// Generate comparison report
    pub fn report(&self) -> String {
        let mut report = String::new();

        report.push_str("=== Performance Comparison Report ===\n\n");

        report.push_str("Baseline:\n");
        report.push_str(&format!("  {}\n\n", self.baseline.summary()));

        report.push_str("Current:\n");
        report.push_str(&format!("  {}\n\n", self.current.summary()));

        if self.has_regression {
            report.push_str("⚠️  Regressions Detected:\n");
            for regression in &self.regressions {
                report.push_str(&format!(
                    "  - {} ({}): {:.1}% ({} -> {})\n",
                    regression.metric_name,
                    match regression.severity {
                        RegressionSeverity::Low => "Low",
                        RegressionSeverity::Medium => "Medium",
                        RegressionSeverity::High => "High",
                        RegressionSeverity::Critical => "Critical",
                    },
                    regression.deviation_percent,
                    regression.baseline_value,
                    regression.current_value
                ));
            }
        } else {
            report.push_str("✅ No regressions detected\n");
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_score() {
        let metrics = AnalysisMetrics {
            planning_time_ms: 5.0,
            execution_time_ms: 50.0,
            startup_time_ms: 2.0,
            total_rows: 1000,
            peak_memory_bytes: 10 * 1024 * 1024,
            throughput: 20000.0,
            cache_hit_rate: 0.9,
            plan_complexity: 5,
            node_stats: vec![],
            timestamp: "2026-06-18T10:00:00Z".to_string(),
        };

        let score = metrics.calculate_score();
        assert!(score > 80.0, "Expected high score for good metrics");
    }

    #[test]
    fn test_comparison() {
        let baseline = AnalysisMetrics {
            planning_time_ms: 5.0,
            execution_time_ms: 50.0,
            startup_time_ms: 2.0,
            total_rows: 1000,
            peak_memory_bytes: 10 * 1024 * 1024,
            throughput: 20000.0,
            cache_hit_rate: 0.9,
            plan_complexity: 5,
            node_stats: vec![],
            timestamp: "2026-06-18T10:00:00Z".to_string(),
        };

        let current = AnalysisMetrics {
            planning_time_ms: 6.0,
            execution_time_ms: 75.0,
            startup_time_ms: 2.0,
            total_rows: 1000,
            peak_memory_bytes: 12 * 1024 * 1024,
            throughput: 13333.0,
            cache_hit_rate: 0.9,
            plan_complexity: 5,
            node_stats: vec![],
            timestamp: "2026-06-18T10:30:00Z".to_string(),
        };

        let comparison = ComparisonResult::new(baseline, current);
        assert!(comparison.has_regression, "Should detect regressions");
    }
}

// benches/analyzer/bottleneck_detector.rs
//! Performance bottleneck detection module

use serde::{Deserialize, Serialize};
use crate::analyzer::AnalysisMetrics;

/// Performance bottleneck types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Bottleneck {
    /// Query planning takes too long
    SlowPlanning {
        time_ms: f64,
        severity: BottleneckSeverity,
    },

    /// Query execution takes too long
    SlowExecution {
        node_id: i64,
        node_name: String,
        time_ms: f64,
        percentage_of_total: f64,
        severity: BottleneckSeverity,
    },

    /// High memory usage
    HighMemory {
        peak_bytes: usize,
        severity: BottleneckSeverity,
    },

    /// Low throughput on a node
    LowThroughput {
        node_id: i64,
        node_name: String,
        rows_per_sec: f64,
        severity: BottleneckSeverity,
    },

    /// High startup latency
    HighStartupLatency {
        time_ms: f64,
        severity: BottleneckSeverity,
    },

    /// Poor cache hit rate
    LowCacheHitRate {
        hit_rate: f64,
        severity: BottleneckSeverity,
    },

    /// Complex execution plan
    ComplexPlan {
        node_count: usize,
        severity: BottleneckSeverity,
    },
}

/// Bottleneck severity levels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum BottleneckSeverity {
    Low,
    Medium,
    High,
    Critical,
}

/// Bottleneck detector
pub struct BottleneckDetector;

impl BottleneckDetector {
    /// Detect all bottlenecks in the analysis metrics
    pub fn detect_all(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        bottlenecks.extend(Self::detect_planning_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_execution_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_memory_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_throughput_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_startup_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_cache_bottlenecks(analysis));
        bottlenecks.extend(Self::detect_plan_complexity_bottlenecks(analysis));

        bottlenecks
    }

    /// Detect planning phase bottlenecks
    fn detect_planning_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        if analysis.planning_time_ms > 100.0 {
            let severity = match analysis.planning_time_ms {
                t if t > 500.0 => BottleneckSeverity::Critical,
                t if t > 300.0 => BottleneckSeverity::High,
                t if t > 150.0 => BottleneckSeverity::Medium,
                _ => BottleneckSeverity::Low,
            };

            bottlenecks.push(Bottleneck::SlowPlanning {
                time_ms: analysis.planning_time_ms,
                severity,
            });
        }

        bottlenecks
    }

    /// Detect execution phase bottlenecks
    fn detect_execution_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        let total_exec_time: f64 = analysis
            .node_stats
            .iter()
            .map(|n| n.execution_time_ms)
            .sum();

        if total_exec_time == 0.0 {
            return bottlenecks;
        }

        for node in &analysis.node_stats {
            let percentage = (node.execution_time_ms / total_exec_time) * 100.0;

            if percentage > 20.0 {
                let severity = match percentage {
                    p if p > 60.0 => BottleneckSeverity::Critical,
                    p if p > 40.0 => BottleneckSeverity::High,
                    p if p > 30.0 => BottleneckSeverity::Medium,
                    _ => BottleneckSeverity::Low,
                };

                bottlenecks.push(Bottleneck::SlowExecution {
                    node_id: node.node_id,
                    node_name: node.node_name.clone(),
                    time_ms: node.execution_time_ms,
                    percentage_of_total: percentage,
                    severity,
                });
            }
        }

        bottlenecks
    }

    /// Detect memory bottlenecks
    fn detect_memory_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        const THRESHOLD_100MB: usize = 100 * 1024 * 1024;

        if analysis.peak_memory_bytes > THRESHOLD_100MB {
            let severity = match analysis.peak_memory_bytes {
                m if m > 500 * 1024 * 1024 => BottleneckSeverity::Critical,
                m if m > 300 * 1024 * 1024 => BottleneckSeverity::High,
                m if m > 200 * 1024 * 1024 => BottleneckSeverity::Medium,
                _ => BottleneckSeverity::Low,
            };

            bottlenecks.push(Bottleneck::HighMemory {
                peak_bytes: analysis.peak_memory_bytes,
                severity,
            });
        }

        bottlenecks
    }

    /// Detect low throughput bottlenecks
    fn detect_throughput_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        const LOW_THROUGHPUT_THRESHOLD: f64 = 1000.0; // rows/sec

        for node in &analysis.node_stats {
            if node.output_rows > 100 && node.throughput_rows_per_sec < LOW_THROUGHPUT_THRESHOLD {
                let severity = match node.throughput_rows_per_sec {
                    t if t < 100.0 => BottleneckSeverity::Critical,
                    t if t < 500.0 => BottleneckSeverity::High,
                    t if t < 800.0 => BottleneckSeverity::Medium,
                    _ => BottleneckSeverity::Low,
                };

                bottlenecks.push(Bottleneck::LowThroughput {
                    node_id: node.node_id,
                    node_name: node.node_name.clone(),
                    rows_per_sec: node.throughput_rows_per_sec,
                    severity,
                });
            }
        }

        bottlenecks
    }

    /// Detect startup latency bottlenecks
    fn detect_startup_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        const STARTUP_THRESHOLD_MS: f64 = 50.0;

        if analysis.startup_time_ms > STARTUP_THRESHOLD_MS {
            let severity = match analysis.startup_time_ms {
                t if t > 200.0 => BottleneckSeverity::Critical,
                t if t > 100.0 => BottleneckSeverity::High,
                t if t > 75.0 => BottleneckSeverity::Medium,
                _ => BottleneckSeverity::Low,
            };

            bottlenecks.push(Bottleneck::HighStartupLatency {
                time_ms: analysis.startup_time_ms,
                severity,
            });
        }

        bottlenecks
    }

    /// Detect cache hit rate bottlenecks
    fn detect_cache_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        const CACHE_THRESHOLD: f64 = 0.6; // 60%

        if analysis.cache_hit_rate < CACHE_THRESHOLD {
            let severity = match analysis.cache_hit_rate {
                r if r < 0.2 => BottleneckSeverity::Critical,
                r if r < 0.4 => BottleneckSeverity::High,
                r if r < 0.6 => BottleneckSeverity::Medium,
                _ => BottleneckSeverity::Low,
            };

            bottlenecks.push(Bottleneck::LowCacheHitRate {
                hit_rate: analysis.cache_hit_rate,
                severity,
            });
        }

        bottlenecks
    }

    /// Detect complex execution plan
    fn detect_plan_complexity_bottlenecks(analysis: &AnalysisMetrics) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];

        const COMPLEXITY_THRESHOLD: usize = 10;

        if analysis.plan_complexity > COMPLEXITY_THRESHOLD {
            let severity = match analysis.plan_complexity {
                c if c > 20 => BottleneckSeverity::Critical,
                c if c > 15 => BottleneckSeverity::High,
                c if c > 12 => BottleneckSeverity::Medium,
                _ => BottleneckSeverity::Low,
            };

            bottlenecks.push(Bottleneck::ComplexPlan {
                node_count: analysis.plan_complexity,
                severity,
            });
        }

        bottlenecks
    }

    /// Get recommendations for a bottleneck
    pub fn get_recommendations(bottleneck: &Bottleneck) -> Vec<String> {
        match bottleneck {
            Bottleneck::SlowPlanning { .. } => vec![
                "Simplify query structure (reduce JOINs, subqueries)".to_string(),
                "Use HINT to guide optimizer".to_string(),
                "Check and update table statistics".to_string(),
                "Consider breaking complex query into multiple simpler ones".to_string(),
            ],
            Bottleneck::SlowExecution {
                node_name, ..
            } => vec![
                format!("Analyze why {} node is slow", node_name),
                "Check if indexes are being used".to_string(),
                "Verify data distribution".to_string(),
                "Consider query rewriting".to_string(),
            ],
            Bottleneck::HighMemory { .. } => vec![
                "Add LIMIT to reduce result set".to_string(),
                "Optimize GROUP BY / DISTINCT operations".to_string(),
                "Use streaming mode if available".to_string(),
                "Consider splitting into multiple queries".to_string(),
            ],
            Bottleneck::LowThroughput { .. } => vec![
                "Check CPU usage during execution".to_string(),
                "Profile hot code paths".to_string(),
                "Optimize inner loop operations".to_string(),
                "Consider using vectorization".to_string(),
            ],
            Bottleneck::HighStartupLatency { .. } => vec![
                "Reduce planning overhead".to_string(),
                "Check system load".to_string(),
                "Warm up caches before execution".to_string(),
                "Use connection pooling".to_string(),
            ],
            Bottleneck::LowCacheHitRate { .. } => vec![
                "Increase cache size".to_string(),
                "Optimize query patterns".to_string(),
                "Warm cache with frequent queries".to_string(),
                "Check cache eviction policy".to_string(),
            ],
            Bottleneck::ComplexPlan { .. } => vec![
                "Break query into smaller steps".to_string(),
                "Use Common Table Expressions (CTEs)".to_string(),
                "Optimize JOIN order".to_string(),
                "Consider materialized views".to_string(),
            ],
        }
    }

    /// Generate detailed bottleneck report
    pub fn generate_report(bottlenecks: &[Bottleneck]) -> String {
        let mut report = String::new();

        if bottlenecks.is_empty() {
            report.push_str("✅ No significant bottlenecks detected\n");
            return report;
        }

        report.push_str("⚠️  Performance Bottlenecks Detected\n");
        report.push_str("=====================================\n\n");

        // Group by severity
        let critical: Vec<_> = bottlenecks
            .iter()
            .filter(|b| b.severity() == BottleneckSeverity::Critical)
            .collect();
        let high: Vec<_> = bottlenecks
            .iter()
            .filter(|b| b.severity() == BottleneckSeverity::High)
            .collect();
        let medium: Vec<_> = bottlenecks
            .iter()
            .filter(|b| b.severity() == BottleneckSeverity::Medium)
            .collect();
        let low: Vec<_> = bottlenecks
            .iter()
            .filter(|b| b.severity() == BottleneckSeverity::Low)
            .collect();

        if !critical.is_empty() {
            report.push_str("🔴 CRITICAL ISSUES\n");
            for bottleneck in critical {
                report.push_str(&format!("  - {}\n", bottleneck.description()));
                for rec in Self::get_recommendations(bottleneck) {
                    report.push_str(&format!("    → {}\n", rec));
                }
            }
            report.push_str("\n");
        }

        if !high.is_empty() {
            report.push_str("🟠 HIGH PRIORITY\n");
            for bottleneck in high {
                report.push_str(&format!("  - {}\n", bottleneck.description()));
                for rec in Self::get_recommendations(bottleneck) {
                    report.push_str(&format!("    → {}\n", rec));
                }
            }
            report.push_str("\n");
        }

        if !medium.is_empty() {
            report.push_str("🟡 MEDIUM PRIORITY\n");
            for bottleneck in medium {
                report.push_str(&format!("  - {}\n", bottleneck.description()));
            }
            report.push_str("\n");
        }

        if !low.is_empty() {
            report.push_str("🔵 LOW PRIORITY\n");
            for bottleneck in low {
                report.push_str(&format!("  - {}\n", bottleneck.description()));
            }
        }

        report
    }
}

impl Bottleneck {
    pub fn severity(&self) -> BottleneckSeverity {
        match self {
            Bottleneck::SlowPlanning { severity, .. } => *severity,
            Bottleneck::SlowExecution { severity, .. } => *severity,
            Bottleneck::HighMemory { severity, .. } => *severity,
            Bottleneck::LowThroughput { severity, .. } => *severity,
            Bottleneck::HighStartupLatency { severity, .. } => *severity,
            Bottleneck::LowCacheHitRate { severity, .. } => *severity,
            Bottleneck::ComplexPlan { severity, .. } => *severity,
        }
    }

    pub fn description(&self) -> String {
        match self {
            Bottleneck::SlowPlanning { time_ms, .. } => {
                format!("Query planning takes {:.2}ms (threshold: 100ms)", time_ms)
            }
            Bottleneck::SlowExecution {
                node_name,
                time_ms,
                percentage_of_total,
                ..
            } => {
                format!(
                    "{} node takes {:.2}ms ({:.1}% of total execution time)",
                    node_name, time_ms, percentage_of_total
                )
            }
            Bottleneck::HighMemory { peak_bytes, .. } => {
                format!(
                    "Peak memory usage is {:.2}MB (threshold: 100MB)",
                    *peak_bytes as f64 / (1024.0 * 1024.0)
                )
            }
            Bottleneck::LowThroughput {
                node_name,
                rows_per_sec,
                ..
            } => {
                format!(
                    "{} node throughput is {:.0} rows/sec (threshold: 1000 rows/sec)",
                    node_name, rows_per_sec
                )
            }
            Bottleneck::HighStartupLatency { time_ms, .. } => {
                format!(
                    "Startup latency is {:.2}ms (threshold: 50ms)",
                    time_ms
                )
            }
            Bottleneck::LowCacheHitRate { hit_rate, .. } => {
                format!(
                    "Cache hit rate is {:.1}% (threshold: 60%)",
                    hit_rate * 100.0
                )
            }
            Bottleneck::ComplexPlan { node_count, .. } => {
                format!(
                    "Execution plan has {} nodes (threshold: 10)",
                    node_count
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_slow_planning() {
        let analysis = AnalysisMetrics {
            planning_time_ms: 200.0,
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

        let bottlenecks = BottleneckDetector::detect_all(&analysis);
        assert!(
            bottlenecks
                .iter()
                .any(|b| matches!(b, Bottleneck::SlowPlanning { .. })),
            "Should detect slow planning"
        );
    }

    #[test]
    fn test_bottleneck_severity() {
        let bottleneck = Bottleneck::SlowPlanning {
            time_ms: 500.0,
            severity: BottleneckSeverity::Critical,
        };

        assert_eq!(bottleneck.severity(), BottleneckSeverity::Critical);
    }
}

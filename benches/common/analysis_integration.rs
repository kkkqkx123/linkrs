// benches/common/analysis_integration.rs
//! Helper functions for integrating performance analysis into existing benchmarks

use crate::analyzer::{AnalysisMetrics, BottleneckDetector};
use std::fs;

/// Save analysis metrics to JSON file
pub fn save_analysis_metrics(metrics: &AnalysisMetrics, output_dir: &str, name: &str) -> std::io::Result<()> {
    // Create output directory if it doesn't exist
    fs::create_dir_all(output_dir)?;

    let json = serde_json::to_string_pretty(metrics)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let filename = format!("{}/{}.json", output_dir, name);
    fs::write(&filename, json)?;

    println!("✅ Metrics saved to: {}", filename);
    Ok(())
}

/// Load baseline metrics from JSON file
pub fn load_baseline_metrics(path: &str) -> std::io::Result<AnalysisMetrics> {
    let content = fs::read_to_string(path)?;
    let metrics = serde_json::from_str(&content)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(metrics)
}

/// Print analysis metrics in tabular format
pub fn print_analysis_metrics(metrics: &AnalysisMetrics) {
    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║           Performance Analysis Results                     ║");
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Planning Phase                                             ║");
    println!("║   Planning Time: {:>47.2}ms ║", metrics.planning_time_ms);
    println!("║   Plan Complexity: {:>44} nodes ║", metrics.plan_complexity);
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Execution Phase                                            ║");
    println!("║   Execution Time: {:>46.2}ms ║", metrics.execution_time_ms);
    println!("║   Startup Time: {:>48.2}ms ║", metrics.startup_time_ms);
    println!("║   Total Rows: {:>50} ║", metrics.total_rows);
    println!("║   Peak Memory: {:>47.2}MB ║",
        metrics.peak_memory_bytes as f64 / (1024.0 * 1024.0));
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Performance Metrics                                        ║");
    println!("║   Throughput: {:>48.0} rows/sec ║", metrics.throughput);
    println!("║   Cache Hit Rate: {:>44.1}% ║", metrics.cache_hit_rate * 100.0);
    println!("║   Performance Score: {:>40.1}/100 ║", metrics.calculate_score());
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();
}

/// Analyze query and print bottlenecks
pub fn analyze_and_print_bottlenecks(metrics: &AnalysisMetrics) {
    let bottlenecks = BottleneckDetector::detect_all(metrics);

    if bottlenecks.is_empty() {
        println!("✅ No significant bottlenecks detected");
        return;
    }

    println!("\n⚠️  Performance Bottlenecks Detected:");
    println!("{}", BottleneckDetector::generate_report(&bottlenecks));
}

/// Detailed analysis report
pub fn print_detailed_analysis_report(metrics: &AnalysisMetrics) {
    println!();
    println!("{}", "=".repeat(70));
    println!("DETAILED PERFORMANCE ANALYSIS REPORT");
    println!("{}", "=".repeat(70));
    println!();
    println!("{}", metrics.detailed_report());

    analyze_and_print_bottlenecks(metrics);
}

/// Quick performance summary for inline reporting
pub fn print_quick_summary(metrics: &AnalysisMetrics) {
    println!("📊 {}", metrics.summary());
}

/// Node statistics table
pub fn print_node_analysis_table(metrics: &AnalysisMetrics) {
    if metrics.node_stats.is_empty() {
        println!("No node statistics available");
        return;
    }

    println!();
    println!("╔════╦═══════════════════╦══════════╦═══════════╦═══════════╦═════════════╗");
    println!("║ ID ║ Node Name         ║ Rows     ║ Time (ms) ║ Memory(KB)║ Rows/sec    ║");
    println!("╠════╬═══════════════════╬══════════╬═══════════╬═══════════╬═════════════╣");

    for node in &metrics.node_stats {
        println!(
            "║ {:2} ║ {:17} ║ {:8} ║ {:9.2} ║ {:9} ║ {:11.0} ║",
            node.node_id,
            node.node_name,
            node.output_rows,
            node.execution_time_ms,
            node.memory_used_bytes / 1024,
            node.throughput_rows_per_sec,
        );
    }

    println!("╚════╩═══════════════════╩══════════╩═══════════╩═══════════╩═════════════╝");
}

/// Regression analysis compared to baseline
pub fn print_regression_analysis(
    baseline: &AnalysisMetrics,
    current: &AnalysisMetrics,
) {
    use crate::analyzer::ComparisonResult;

    let comparison = ComparisonResult::new(baseline.clone(), current.clone());

    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║         BASELINE vs CURRENT COMPARISON                    ║");
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Baseline Score:  {:>48.1}/100 ║", baseline.calculate_score());
    println!("║ Current Score:   {:>48.1}/100 ║", current.calculate_score());
    println!("╠════════════════════════════════════════════════════════════╣");

    for (metric_name, deviation) in &comparison.deviations {
        let status = if deviation.abs() < 5.0 {
            "✅"
        } else if deviation.abs() < 15.0 {
            "⚠️"
        } else {
            "🔴"
        };

        println!(
            "║ {} {:<40} {:>12.1}% ║",
            status, metric_name, deviation
        );
    }

    println!("╚════════════════════════════════════════════════════════════╝");

    if comparison.has_regression {
        println!("\n⚠️ REGRESSIONS DETECTED:");
        for regression in &comparison.regressions {
            println!(
                "  {} {}: {:.1}% worse ({} -> {})",
                match regression.severity {
                    crate::analyzer::metrics::RegressionSeverity::Critical => "🔴",
                    crate::analyzer::metrics::RegressionSeverity::High => "🟠",
                    crate::analyzer::metrics::RegressionSeverity::Medium => "🟡",
                    crate::analyzer::metrics::RegressionSeverity::Low => "🔵",
                },
                regression.metric_name,
                regression.deviation_percent,
                regression.baseline_value,
                regression.current_value,
            );
        }
    } else {
        println!("\n✅ No regressions detected - performance is stable");
    }
}

/// Performance scoring helper
pub fn score_to_grade(score: f64) -> &'static str {
    match score {
        s if s >= 90.0 => "A (Excellent)",
        s if s >= 80.0 => "B (Very Good)",
        s if s >= 70.0 => "C (Good)",
        s if s >= 60.0 => "D (Acceptable)",
        s if s >= 50.0 => "E (Needs Improvement)",
        _ => "F (Poor)",
    }
}

/// Print performance grade
pub fn print_performance_grade(metrics: &AnalysisMetrics) {
    let score = metrics.calculate_score();
    let grade = score_to_grade(score);

    println!();
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║         PERFORMANCE GRADE                                  ║");
    println!("╠════════════════════════════════════════════════════════════╣");
    println!("║ Score: {:>57.1}/100 ║", score);
    println!("║ Grade: {:>57} ║", grade);
    println!("╚════════════════════════════════════════════════════════════╝");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisMetrics, NodeMetrics};

    #[test]
    fn test_score_to_grade() {
        assert_eq!(score_to_grade(95.0), "A (Excellent)");
        assert_eq!(score_to_grade(85.0), "B (Very Good)");
        assert_eq!(score_to_grade(75.0), "C (Good)");
        assert_eq!(score_to_grade(65.0), "D (Acceptable)");
        assert_eq!(score_to_grade(55.0), "E (Needs Improvement)");
        assert_eq!(score_to_grade(45.0), "F (Poor)");
    }

    #[test]
    fn test_save_and_load_metrics() -> std::io::Result<()> {
        let metrics = AnalysisMetrics {
            planning_time_ms: 5.32,
            execution_time_ms: 48.24,
            startup_time_ms: 2.15,
            total_rows: 1000,
            peak_memory_bytes: 256 * 1024,
            throughput: 20745.0,
            cache_hit_rate: 0.9,
            plan_complexity: 5,
            node_stats: vec![],
            timestamp: "2026-06-18T10:00:00Z".to_string(),
        };

        // Create temp directory
        let temp_dir = "/tmp/bench_analysis_test";
        fs::create_dir_all(temp_dir)?;

        // Save
        save_analysis_metrics(&metrics, temp_dir, "test_metrics")?;

        // Load
        let loaded = load_baseline_metrics(&format!("{}/test_metrics.json", temp_dir))?;

        assert_eq!(loaded.planning_time_ms, 5.32);
        assert_eq!(loaded.total_rows, 1000);

        // Cleanup
        fs::remove_dir_all(temp_dir)?;

        Ok(())
    }
}

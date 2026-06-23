// benches/analyzer/performance_analyzer.rs
//! Performance analyzer for benchmark analysis using EXPLAIN and PROFILE

use crate::analyzer::{AnalysisMetrics, BottleneckDetector, NodeMetrics};
use regex::Regex;
use std::time::SystemTime;

/// Performance analyzer
pub struct PerformanceAnalyzer;

impl PerformanceAnalyzer {
    /// Parse EXPLAIN ANALYZE output to extract metrics
    ///
    /// Expected format:
    /// ```
    /// Explain Analyze for query: MATCH (n:Data) RETURN n
    /// ───────────────────────────────────────────────────
    ///  id | name    | output | execution_time | rows | memory
    /// ────┼─────────┼────────┼────────────────┼──────┼──────
    ///  0  | Project | [n]    | 0.45 ms        | 1000 | 256KB
    ///  1  | Filter  | [n]    | 2.12 ms        | 500  | 128KB
    ///  2  | Scan    | [n]    | 45.67 ms       | 1000 | 512KB
    /// ────┴─────────┴────────┴────────────────┴──────┴──────
    ///
    /// Planning Time: 5.32 ms
    /// Execution Time: 48.24 ms
    /// Total Rows: 1000
    /// Peak Memory: 512 KB
    /// ```
    pub fn parse_explain_analyze_output(output: &str) -> Result<AnalysisMetrics, String> {
        let mut planning_time_ms = 0.0;
        let mut execution_time_ms = 0.0;
        let mut total_rows = 0usize;
        let mut peak_memory_bytes = 0usize;
        let mut node_stats = vec![];

        // Extract planning time
        if let Some(planning_time) = Self::extract_float_value(output, "Planning Time:") {
            planning_time_ms = planning_time;
        }

        // Extract execution time
        if let Some(execution_time) = Self::extract_float_value(output, "Execution Time:") {
            execution_time_ms = execution_time;
        }

        // Extract total rows
        if let Some(rows) = Self::extract_usize_value(output, "Total Rows:") {
            total_rows = rows;
        }

        // Extract peak memory
        if let Some(memory) = Self::extract_memory_value(output, "Peak Memory:") {
            peak_memory_bytes = memory;
        }

        // Parse node statistics from table
        node_stats = Self::parse_node_stats_table(output);

        // Calculate derived metrics
        let startup_time_ms = if !node_stats.is_empty() {
            node_stats[0].execution_time_ms / 2.0 // Approximate
        } else {
            0.0
        };

        let throughput = if execution_time_ms > 0.0 {
            (total_rows as f64 / execution_time_ms) * 1000.0
        } else {
            0.0
        };

        let plan_complexity = node_stats.len();

        Ok(AnalysisMetrics {
            planning_time_ms,
            execution_time_ms,
            startup_time_ms,
            total_rows,
            peak_memory_bytes,
            throughput,
            cache_hit_rate: 0.0,
            plan_complexity,
            node_stats,
            timestamp: format!("{:?}", SystemTime::now()),
        })
    }

    /// Parse node statistics table from EXPLAIN ANALYZE output
    fn parse_node_stats_table(output: &str) -> Vec<NodeMetrics> {
        let mut nodes = vec![];
        let lines: Vec<&str> = output.lines().collect();

        for (idx, line) in lines.iter().enumerate() {
            // Look for lines that contain node statistics (contain |)
            if !line.contains('|') {
                continue;
            }

            // Skip header lines
            if line.contains("name") || line.contains("───") {
                continue;
            }

            // Parse node line
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() < 5 {
                continue;
            }

            if let Ok(node_id) = parts[0].trim().parse::<i64>() {
                let node_name = parts[1].trim().to_string();
                let execution_time_str = parts[3].trim();
                let rows_str = parts[4].trim();

                if let Ok(execution_time_ms) = Self::parse_time(execution_time_str) {
                    if let Ok(output_rows) = rows_str.parse::<usize>() {
                        let memory_used_bytes = if parts.len() > 5 {
                            Self::parse_memory(parts[5].trim()).unwrap_or(0)
                        } else {
                            0
                        };

                        let throughput_rows_per_sec = if execution_time_ms > 0.0 {
                            (output_rows as f64 / execution_time_ms) * 1000.0
                        } else {
                            0.0
                        };

                        nodes.push(NodeMetrics {
                            node_id,
                            node_name,
                            output_rows,
                            execution_time_ms,
                            memory_used_bytes,
                            throughput_rows_per_sec,
                        });
                    }
                }
            }
        }

        nodes
    }

    /// Extract floating point value after a label
    fn extract_float_value(output: &str, label: &str) -> Option<f64> {
        let re = Regex::new(&format!(r"{}\s*([\d.]+)", regex::escape(label)))
            .ok()?;

        re.captures(output)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<f64>().ok())
    }

    /// Extract unsigned integer value after a label
    fn extract_usize_value(output: &str, label: &str) -> Option<usize> {
        let re = Regex::new(&format!(r"{}\s*(\d+)", regex::escape(label)))
            .ok()?;

        re.captures(output)
            .and_then(|cap| cap.get(1))
            .and_then(|m| m.as_str().parse::<usize>().ok())
    }

    /// Extract memory value after a label (supports KB, MB, GB)
    fn extract_memory_value(output: &str, label: &str) -> Option<usize> {
        let re = Regex::new(&format!(r"{}\s*([\d.]+)\s*([KMGT]B)", regex::escape(label)))
            .ok()?;

        let caps = re.captures(output)?;
        let value = caps.get(1)?.as_str().parse::<f64>().ok()?;
        let unit = caps.get(2)?.as_str();

        let bytes = match unit {
            "KB" => (value * 1024.0) as usize,
            "MB" => (value * 1024.0 * 1024.0) as usize,
            "GB" => (value * 1024.0 * 1024.0 * 1024.0) as usize,
            "B" => value as usize,
            _ => return None,
        };

        Some(bytes)
    }

    /// Parse time value (supports ms, s, us)
    fn parse_time(time_str: &str) -> Result<f64, String> {
        let re = Regex::new(r"([\d.]+)\s*(ms|s|us)")
            .map_err(|e| format!("Regex error: {}", e))?;

        let caps = re
            .captures(time_str)
            .ok_or_else(|| format!("Cannot parse time: {}", time_str))?;

        let value = caps
            .get(1)
            .ok_or("Missing time value")?
            .as_str()
            .parse::<f64>()
            .map_err(|e| format!("Parse error: {}", e))?;

        let unit = caps.get(2).ok_or("Missing unit")?.as_str();

        let ms = match unit {
            "ms" => value,
            "s" => value * 1000.0,
            "us" => value / 1000.0,
            _ => return Err(format!("Unknown time unit: {}", unit)),
        };

        Ok(ms)
    }

    /// Parse memory value
    fn parse_memory(memory_str: &str) -> Result<usize, String> {
        let re = Regex::new(r"([\d.]+)\s*([KMGT]B)")
            .map_err(|e| format!("Regex error: {}", e))?;

        let caps = re
            .captures(memory_str)
            .ok_or_else(|| format!("Cannot parse memory: {}", memory_str))?;

        let value = caps
            .get(1)
            .ok_or("Missing memory value")?
            .as_str()
            .parse::<f64>()
            .map_err(|e| format!("Parse error: {}", e))?;

        let unit = caps.get(2).ok_or("Missing unit")?.as_str();

        let bytes = match unit {
            "KB" => (value * 1024.0) as usize,
            "MB" => (value * 1024.0 * 1024.0) as usize,
            "GB" => (value * 1024.0 * 1024.0 * 1024.0) as usize,
            "B" => value as usize,
            _ => return Err(format!("Unknown memory unit: {}", unit)),
        };

        Ok(bytes)
    }

    /// Analyze metrics and detect bottlenecks
    pub fn analyze_bottlenecks(metrics: &AnalysisMetrics) -> Vec<String> {
        let bottlenecks = BottleneckDetector::detect_all(metrics);

        let mut recommendations = vec![];
        for bottleneck in &bottlenecks {
            recommendations.push(bottleneck.description());
            let recs = BottleneckDetector::get_recommendations(bottleneck);
            for rec in recs {
                recommendations.push(format!("  → {}", rec));
            }
        }

        recommendations
    }

    /// Generate a comprehensive analysis report
    pub fn generate_report(metrics: &AnalysisMetrics) -> String {
        let mut report = String::new();

        report.push_str("═══════════════════════════════════════════════════════════\n");
        report.push_str("        Performance Analysis Report\n");
        report.push_str("═══════════════════════════════════════════════════════════\n\n");

        report.push_str(&metrics.detailed_report());

        let bottlenecks = BottleneckDetector::detect_all(metrics);
        report.push_str("\n");
        report.push_str(&BottleneckDetector::generate_report(&bottlenecks));

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time() {
        assert_eq!(PerformanceAnalyzer::parse_time("5.32 ms").unwrap(), 5.32);
        assert_eq!(PerformanceAnalyzer::parse_time("2.0 s").unwrap(), 2000.0);
        assert_eq!(PerformanceAnalyzer::parse_time("1000 us").unwrap(), 1.0);
    }

    #[test]
    fn test_parse_memory() {
        assert_eq!(PerformanceAnalyzer::parse_memory("512 KB").unwrap(), 512 * 1024);
        assert_eq!(
            PerformanceAnalyzer::parse_memory("1.5 MB").unwrap(),
            (1.5 * 1024.0 * 1024.0) as usize
        );
    }

    #[test]
    fn test_extract_float_value() {
        let output = "Planning Time: 5.32 ms";
        let value = PerformanceAnalyzer::extract_float_value(output, "Planning Time:").unwrap();
        assert_eq!(value, 5.32);
    }

    #[test]
    fn test_extract_usize_value() {
        let output = "Total Rows: 1000";
        let value = PerformanceAnalyzer::extract_usize_value(output, "Total Rows:").unwrap();
        assert_eq!(value, 1000);
    }
}

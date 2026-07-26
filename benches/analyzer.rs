// benches/analyzer/mod.rs
//! Performance analysis module for benchmark analysis
//! Provides tools for analyzing query performance using EXPLAIN ANALYZE and PROFILE

#[path = "analyzer/bottleneck_detector.rs"]
pub mod bottleneck_detector;
#[path = "analyzer/metrics.rs"]
pub mod metrics;
#[path = "analyzer/performance_analyzer.rs"]
pub mod performance_analyzer;

pub use bottleneck_detector::{Bottleneck, BottleneckDetector, BottleneckSeverity};
pub use metrics::{AnalysisMetrics, ComparisonResult, NodeMetrics};
pub use performance_analyzer::PerformanceAnalyzer;

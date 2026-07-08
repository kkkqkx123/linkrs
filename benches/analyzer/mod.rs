// benches/analyzer/mod.rs
//! Performance analysis module for benchmark analysis
//! Provides tools for analyzing query performance using EXPLAIN ANALYZE and PROFILE

pub mod metrics;
pub mod performance_analyzer;
pub mod bottleneck_detector;

pub use metrics::{AnalysisMetrics, NodeMetrics, ComparisonResult};
pub use performance_analyzer::PerformanceAnalyzer;
pub use bottleneck_detector::{Bottleneck, BottleneckDetector, BottleneckSeverity};

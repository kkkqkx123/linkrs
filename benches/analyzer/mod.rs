// benches/analyzer/mod.rs
//! Performance analysis module for benchmark analysis
//! Provides tools for analyzing query performance using EXPLAIN ANALYZE and PROFILE

pub mod bottleneck_detector;
pub mod metrics;
pub mod performance_analyzer;

pub use bottleneck_detector::{Bottleneck, BottleneckDetector, BottleneckSeverity};
pub use metrics::{AnalysisMetrics, ComparisonResult, NodeMetrics};
pub use performance_analyzer::PerformanceAnalyzer;

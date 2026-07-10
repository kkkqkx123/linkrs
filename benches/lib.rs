// benches/lib.rs

pub mod analyzer;
pub mod common;

pub use analyzer::{AnalysisMetrics, BottleneckDetector, PerformanceAnalyzer};
pub use common::{BenchmarkContext, BenchmarkDataStats, DataGenerator};

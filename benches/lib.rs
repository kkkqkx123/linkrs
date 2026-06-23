// benches/lib.rs

pub mod analyzer;
pub mod common;

pub use analyzer::{AnalysisMetrics, PerformanceAnalyzer, BottleneckDetector};
pub use common::{DataGenerator, BenchmarkContext, BenchmarkDataStats};

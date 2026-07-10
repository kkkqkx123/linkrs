//! Pipeline DAG analysis and execution
//!
//! This module implements Phase 6 of the executor evolution:
//! decomposing the physical plan into a DAG of pipelines separated
//! by breaker operators.
//!
//! Phase 6a focus: pipeline analysis, graph types, explain output,
//! and single-threaded pipeline runner (no parallelism yet).

pub mod analyzer;
pub mod breaker;
pub mod graph;
pub mod runner;

pub use analyzer::PipelineAnalyzer;
pub use breaker::PipelineBreakerKind;
pub use graph::{Pipeline, PipelineGraph, PipelineSink, PipelineSource};
pub use runner::PipelineRunner;

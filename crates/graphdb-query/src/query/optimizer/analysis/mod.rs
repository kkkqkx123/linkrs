//! Plan Analysis Module
//!
//! Provides an analysis of query plans, supporting decision-making processes related to optimization.
//! Quote Count Analysis: Identifying Sub-plans That Are Cited Multiple Times
//! Expression analysis: Examining the characteristics of an expression (such as determinacy, complexity, etc.)
//! Fingerprint calculation: Calculating the structural fingerprint of the planning nodes
//!
//! # Explanation of the module locations
//!
//! This module is located in `src/query/optimizer/analysis/` and is at the same level as the `cost` module.
//! Reason for placing it in the optimizer rather than the planner:
//! Separation of responsibilities: The planner is responsible for generating plans, while the optimizer is responsible for optimizing these plans.
//! 2. Calculation on demand: Analysis is only performed during the optimization phase, as and when necessary.
//! 3. Dependency relationships: The optimizer already depends on the planner; therefore, no circular dependencies will be introduced.
//!
//! # Usage Examples
//!
//! ```rust
//! use crate::query::optimizer::analysis::{
//!     ReferenceCountAnalyzer,
//!     ExpressionAnalyzer,
//! };
//!
// Analysis of reference count statistics
//! let ref_analyzer = ReferenceCountAnalyzer::new();
//! let ref_analysis = ref_analyzer.analyze(plan.root());
//!
// Expression analysis
//! let expr_analyzer = ExpressionAnalyzer::new();
//! let expr_analysis = expr_analyzer.analyze(condition);
//! ```

pub mod batch;
pub mod expression;
pub mod fingerprint;
pub mod reference_count;

// Re-export the main types
pub use batch::{AggregatedExpressionAnalysis, BatchPlanAnalysis, BatchPlanAnalyzer};
pub use expression::{
    AnalysisMode, AnalysisOptions, ExpressionAnalysis, ExpressionAnalyzer, NondeterministicChecker,
};
pub use fingerprint::{FingerprintCalculator, PlanFingerprint};
pub use reference_count::{
    ReferenceCountAnalysis, ReferenceCountAnalyzer, SubplanId, SubplanReferenceInfo,
};

#[cfg(test)]
mod integration_test;

//! Optimized Decision-Making Module
//!
//! Provide definitions for the types of optimization decisions.
//!
//! The DecisionCache that was originally included has been deleted because:
//! The computational cost for decision-making is not high; however, the benefits of caching are limited.
//! The version-awareness mechanism is complex, and the maintenance costs are high.
//! 3. There are few practical use cases; QueryPlanCache is already sufficient.

// Type definition
pub mod types;

// Re-export the main types
pub use types::{
    AccessPath, EntityIndexChoice, EntityType, IndexChoice, IndexSelectionDecision, JoinAlgorithm,
    JoinOrderDecision, OptimizationDecision, RewriteRuleId, TraversalStartDecision,
};

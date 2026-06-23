//! Heuristic Optimization Module
//!
//! This module contains all the heuristic optimization rules, which are applied directly during the plan generation phase.
//! These rules do not rely on cost calculations; they always generate better or equivalent plans.
//!
//! # Module Structure
//!
//! ## Core Components
//!
//! - `context`: Rewrite context definition
//! - `pattern`: Pattern matching definitions
//! - `result`: Rewrite result types
//! - `rule`: Rewrite rule trait definitions
//! - `macros`: Macro definitions for rewriting rules
//! - `rewrite_rule`: Rewrite rule trait and adapters
//! - `plan_rewriter`: Plan rewriter implementation
//! - `visitor`: Node visitor for tree traversal
//!
//! ## Rule Categories
//!
//! ### Predicate Pushdown Rules
//! Push filtering conditions to the lowest level of the planning tree to reduce data processing volume.
//!
//! ### Projection Pushdown Rules
//! Push projection operations down to reduce data transfer between operators.
//!
//! ### Merge Rules
//! Merge multiple consecutive operations of the same type to reduce intermediate results.
//!
//! ### Elimination Rules
//! Remove redundant operations:
//! - `EliminateFilterRule`: Remove tautological filters
//! - `RemoveNoopProjectRule`: Remove no-operation projections
//! - `DedupEliminationRule`: Remove unnecessary deduplication
//! - `EliminateSortRule`: Remove redundant sorting
//!
//! ### Limit Pushdown Rules
//! Push LIMIT/TOPN operations down to reduce result set size early.
//!
//! ### Aggregate Optimization Rules
//! Optimize aggregate operations.
//!
//! # Relationship with Cost-Based Optimization
//!
//! The rules in this module are **heuristic rules** that do not rely on cost calculations and are always executed.
//! Cost-based optimization is implemented in the `cost_based` module, which includes:
//! - `SortEliminationOptimizer`: Decides whether to convert Sort+Limit to TopN based on cost
//! - `AggregateStrategySelector`: Selects optimal aggregation strategy
//! - `JoinOrderOptimizer`: Optimizes join order using dynamic programming
//! - `TraversalDirectionOptimizer`: Selects optimal traversal direction
//! - `SubqueryUnnestingOptimizer`: Transforms correlated subqueries
//!
//! Heuristic rules are executed first (Phase 1), followed by cost-based optimization (Phase 2).
//!
//! # Usage Examples
//!
//! ```rust
//! use crate::query::optimizer::heuristic::{PlanRewriter, create_default_rewriter, rewrite_plan};
//! use crate::query::planning::plan::ExecutionPlan;
//!
//! // Use the default rewriter
//! let plan = ExecutionPlan::new(...);
//! let optimized_plan = rewrite_plan(plan)?;
//!
//! // Custom rewriter
//! let mut rewriter = PlanRewriter::new();
//! rewriter.add_rule(MyCustomRule);
//! let optimized_plan = rewriter.rewrite(plan)?;
//! ```

// Core Type Modules (New)
pub mod context;
pub mod expression_utils;
pub mod pattern;
pub mod result;
pub mod rule;
pub mod visitor;

// Macro module
pub mod macros;

// Core trait and implementation
pub mod plan_rewriter;
pub mod rewrite_rule;

// Enumeration of static distribution rules
pub mod rule_enum;

// Specific Rules Module
pub mod aggregate;
pub mod elimination;
pub mod join_optimization;
pub mod limit_pushdown;
pub mod merge;
pub mod predicate_pushdown;
pub mod projection_pushdown;

// ==================== Exporting Core Types =====================

// Export from the new independent module.
pub use context::RewriteContext;
pub use pattern::{
    MatchNode, NodeVisitor, NodeVisitorFinder, NodeVisitorRecorder, Pattern, PlanNodeMatcher,
};
pub use result::{MatchedResult, RewriteError, RewriteResult, TransformResult};
pub use rule::{
    BaseRewriteRule, EliminationRule, IntoRuleWrapper, MergeRule, PushDownRule, RewriteRule,
    RuleWrapper,
};
pub use visitor::ChildRewriteVisitor;

// Export from the compatibility layer
pub use rewrite_rule::{HeuristicRule, HeuristicRuleAdapter, IntoOptRule};

pub use plan_rewriter::{create_default_rewriter, rewrite_plan, PlanRewriter};

// Export the enumeration of static distribution rules.
pub use rule_enum::{RewriteRule as RewriteRuleEnum, RuleRegistry};

// Export all rewriting rules in a unified manner.
pub use aggregate::*;
pub use elimination::*;
pub use join_optimization::*;
pub use limit_pushdown::*;
pub use merge::*;
pub use predicate_pushdown::*;
pub use projection_pushdown::*;

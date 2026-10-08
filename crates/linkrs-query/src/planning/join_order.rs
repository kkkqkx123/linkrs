//! Join order enumeration foundation.
//!
//! This module hosts the DP-based join order infrastructure behind
//! worst-case optimal join planning: the pattern [`QueryGraph`], the bitset
//! [`SubqueryGraph`], the DP [`SubPlansTable`], the [`CostModel`], the
//! [`CardinalityEstimator`], the [`JoinOrderEnumerator`], and the join-hint
//! [`JoinTree`] support.
//!
//! Status: fully implemented and wired into MATCH planning.
//! `MatchStatementPlanner::try_join_order_plan` routes conjunctive MATCH
//! patterns through `JoinOrderEnumerator`, which runs DP enumeration over
//! an explicitly built `QueryGraph` and generates `LogicalWcoIntersect`
//! nodes when WCO is cost-optimal. `plan_with_hint` solves a programmatic
//! `JoinHint` via `USING JOIN BINARY/MULTIWAY` syntax. Physical lowering
//! maps `LogicalWcoIntersect` to a dedicated `WcoIntersectNode`, and the
//! streaming `WcoIntersectOperator` executes sorted-merge intersection
//! over sealed build tables.

pub mod cardinality_estimator;
pub mod cost_model;
pub mod from_pattern;
pub mod join_tree;
pub mod plan_intersect;
pub mod plan_join_order;
pub mod query_graph;
pub mod subplans_table;
pub mod subquery_graph;

pub use cardinality_estimator::{CardinalityEstimator, JoinOrderStats};
pub use cost_model::CostModel;
pub use from_pattern::query_graph_from_match_patterns;
pub use join_tree::{
    JoinHint, JoinHintError, JoinHintNode, JoinTree, JoinTreeConstructor, JoinTreeExtraInfo,
    JoinTreeNode, TreeNodeType,
};
pub use plan_join_order::{
    JoinOrderEnumerator, JoinOrderEnumeratorContext, MAX_LEVEL_TO_PLAN_EXACTLY,
};
pub use query_graph::{ExtendDirection, QueryGraph, QueryNode, QueryRel};
pub use subplans_table::{JoinOrderPlan, SubPlansTable, SubgraphPlans};
pub use subquery_graph::{SubqueryGraph, MAX_NUM_QUERY_VARIABLES};

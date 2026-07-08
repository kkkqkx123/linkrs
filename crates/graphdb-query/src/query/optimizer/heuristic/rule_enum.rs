//! Rewrite rule enumeration – Implementation of static distribution
//!
//! This module uses enumerations to implement static distribution, thereby avoiding the overhead associated with dynamic distribution.
//! All rules are distributed in the form of enumeration variants, using the `match` mechanism.
//!
//! # Advantages
//!
//! No dynamic distribution overhead (no lookup in virtual function tables).
//! No heap allocation (the rules are stored on the stack).
//! Better cache locality
//! Compilers can perform inlining optimizations.
//!
//! # Usage Examples
//!
//! ```rust
//! use crate::query::optimizer::heuristic::rule_enum::{RewriteRule, RuleRegistry};
//!
// Create the rule registry
//! let registry = RuleRegistry::default();
//!
// Application rules
//! for rule in registry.iter() {
//!     if let Some(result) = rule.apply(ctx, node)? {
// Processing the results
//!     }
//! }
//! ```

use crate::query::optimizer::heuristic::aggregate;
use crate::query::optimizer::heuristic::context::RewriteContext;
use crate::query::optimizer::heuristic::elimination;
use crate::query::optimizer::heuristic::join_optimization;
use crate::query::optimizer::heuristic::limit_pushdown;
use crate::query::optimizer::heuristic::merge;
use crate::query::optimizer::heuristic::pattern::Pattern;
use crate::query::optimizer::heuristic::predicate_pushdown;
use crate::query::optimizer::heuristic::projection_pushdown;
use crate::query::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::query::optimizer::heuristic::rule::RewriteRule as RewriteRuleTrait;
use crate::query::planning::plan::PlanNodeEnum;

macro_rules! define_rewrite_rules {
    (
        $(#[$enum_meta:meta])*
        pub enum $enum_name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant_name:ident($rule_type:ty)
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug)]
        pub enum $enum_name {
            $(
                $(#[$variant_meta])*
                $variant_name($rule_type),
            )+
        }

        impl $enum_name {
            pub fn name(&self) -> &'static str {
                match self {
                    $(
                        $enum_name::$variant_name(_) => {
                            let type_name = stringify!($rule_type);
                            if let Some(pos) = type_name.rfind("::") {
                                &type_name[pos + 2..]
                            } else {
                                type_name
                            }
                        }
                    )+
                }
            }

            pub fn pattern(&self) -> Pattern {
                match self {
                    $(
                        $enum_name::$variant_name(rule) => rule.pattern(),
                    )+
                }
            }

            pub fn apply(
                &self,
                ctx: &mut RewriteContext,
                node: &PlanNodeEnum,
            ) -> RewriteResult<Option<TransformResult>> {
                match self {
                    $(
                        $enum_name::$variant_name(rule) => rule.apply(ctx, node),
                    )+
                }
            }

            pub fn matches(&self, node: &PlanNodeEnum) -> bool {
                self.pattern().matches(node)
            }
        }

        impl RewriteRuleTrait for $enum_name {
            fn name(&self) -> &'static str {
                self.name()
            }

            fn pattern(&self) -> Pattern {
                self.pattern()
            }

            fn apply(
                &self,
                ctx: &mut RewriteContext,
                node: &PlanNodeEnum,
            ) -> RewriteResult<Option<TransformResult>> {
                self.apply(ctx, node)
            }
        }
    };
}

define_rewrite_rules! {
    pub enum RewriteRule {
        // ==================== Remove Rules ====================
        EliminateFilter(elimination::EliminateFilterRule),
        RemoveNoopProject(elimination::RemoveNoopProjectRule),
        EliminateAppendVertices(elimination::EliminateAppendVerticesRule),
        RemoveAppendVerticesBelowJoin(elimination::RemoveAppendVerticesBelowJoinRule),
        EliminateRowCollect(elimination::EliminateRowCollectRule),
        EliminateEmptySetOperation(elimination::EliminateEmptySetOperationRule),
        DedupElimination(elimination::DedupEliminationRule),
        EliminateSort(elimination::EliminateSortRule),

        // ==================== Merging Rules ====================
        CombineFilter(merge::CombineFilterRule),
        CollapseProject(merge::CollapseProjectRule),
        CollapseConsecutiveProject(merge::CollapseConsecutiveProjectRule),
        MergeGetVerticesAndProject(merge::MergeGetVerticesAndProjectRule),
        MergeGetVerticesAndDedup(merge::MergeGetVerticesAndDedupRule),
        MergeGetNbrsAndProject(merge::MergeGetNbrsAndProjectRule),
        MergeGetNbrsAndDedup(merge::MergeGetNbrsAndDedupRule),

        // ==================== Predicate Pushdown Rules ====================
        PushFilterDownTraverse(predicate_pushdown::PushFilterDownTraverseRule),
        PushFilterDownExpandAll(predicate_pushdown::PushFilterDownExpandAllRule),
        PushFilterDownNode(predicate_pushdown::PushFilterDownNodeRule),
        PushEFilterDown(predicate_pushdown::PushEFilterDownRule),
        PushVFilterDownScanVertices(predicate_pushdown::PushVFilterDownScanVerticesRule),
        PushFilterDownInnerJoin(predicate_pushdown::PushFilterDownInnerJoinRule),
        PushFilterDownHashInnerJoin(predicate_pushdown::PushFilterDownHashInnerJoinRule),
        PushFilterDownHashLeftJoin(predicate_pushdown::PushFilterDownHashLeftJoinRule),
        PushFilterDownCrossJoin(predicate_pushdown::PushFilterDownCrossJoinRule),
        PushFilterDownGetNbrs(predicate_pushdown::PushFilterDownGetNbrsRule),
        PushFilterDownAllPaths(predicate_pushdown::PushFilterDownAllPathsRule),

        // ==================== Projection Pushdown Rules ====================
        PushProjectDownScanVertices(projection_pushdown::PushProjectDownScanVerticesRule),
        PushProjectDownScanEdges(projection_pushdown::PushProjectDownScanEdgesRule),
        PushProjectDownGetVertices(projection_pushdown::PushProjectDownGetVerticesRule),
        PushProjectDownGetEdges(projection_pushdown::PushProjectDownGetEdgesRule),
        PushProjectDownGetNeighbors(projection_pushdown::PushProjectDownGetNeighborsRule),
        PushProjectDownEdgeIndexScan(projection_pushdown::PushProjectDownEdgeIndexScanRule),

        // ==================== Rules for Pushing Limits Down ====================
        PushLimitDownGetVertices(limit_pushdown::PushLimitDownGetVerticesRule),
        PushLimitDownGetEdges(limit_pushdown::PushLimitDownGetEdgesRule),
        PushLimitDownScanVertices(limit_pushdown::PushLimitDownScanVerticesRule),
        PushLimitDownScanEdges(limit_pushdown::PushLimitDownScanEdgesRule),
        PushLimitDownIndexScan(limit_pushdown::PushLimitDownIndexScanRule),
        PushTopNDownIndexScan(limit_pushdown::PushTopNDownIndexScanRule),
        ConvertSortLimitToTopN(limit_pushdown::ConvertSortLimitToTopNRule),

        // ==================== Aggregation Optimization Rules ====================
        PushFilterDownAggregate(aggregate::PushFilterDownAggregateRule),

        // ==================== JOIN Optimization Rules ====================
        PushProjectDownJoin(join_optimization::PushProjectDownJoinRule),
        LeftJoinToInnerJoin(join_optimization::LeftJoinToInnerJoinRule),
        JoinConditionSimplify(join_optimization::JoinConditionSimplifyRule),
        JoinToExpand(join_optimization::JoinToExpandRule),
        JoinToAppendVertices(join_optimization::JoinToAppendVerticesRule),
        MergeConsecutiveExpand(join_optimization::MergeConsecutiveExpandRule),
        JoinElimination(join_optimization::JoinEliminationRule),
        IndexJoinSelection(join_optimization::IndexJoinSelectionRule),
        JoinReorder(join_optimization::JoinReorderRule),
    }
}

#[derive(Debug)]
pub struct RuleRegistry {
    rules: Vec<RewriteRule>,
}

impl RuleRegistry {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn add(&mut self, rule: RewriteRule) {
        self.rules.push(rule);
    }

    pub fn iter(&self) -> impl Iterator<Item = &RewriteRule> {
        self.rules.iter()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn clear(&mut self) {
        self.rules.clear();
    }

    pub fn into_vec(self) -> Vec<RewriteRule> {
        self.rules
    }
}

impl Default for RuleRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.add(RewriteRule::EliminateFilter(
            elimination::EliminateFilterRule::new(),
        ));
        registry.add(RewriteRule::RemoveNoopProject(
            elimination::RemoveNoopProjectRule::new(),
        ));
        registry.add(RewriteRule::EliminateAppendVertices(
            elimination::EliminateAppendVerticesRule::new(),
        ));
        registry.add(RewriteRule::RemoveAppendVerticesBelowJoin(
            elimination::RemoveAppendVerticesBelowJoinRule::new(),
        ));
        registry.add(RewriteRule::EliminateRowCollect(
            elimination::EliminateRowCollectRule::new(),
        ));
        registry.add(RewriteRule::EliminateEmptySetOperation(
            elimination::EliminateEmptySetOperationRule::new(),
        ));
        registry.add(RewriteRule::DedupElimination(
            elimination::DedupEliminationRule::new(),
        ));
        registry.add(RewriteRule::EliminateSort(
            elimination::EliminateSortRule::new(),
        ));
        registry.add(RewriteRule::CombineFilter(merge::CombineFilterRule::new()));
        registry.add(RewriteRule::CollapseProject(
            merge::CollapseProjectRule::new(),
        ));
        registry.add(RewriteRule::CollapseConsecutiveProject(
            merge::CollapseConsecutiveProjectRule::new(),
        ));
        registry.add(RewriteRule::MergeGetVerticesAndProject(
            merge::MergeGetVerticesAndProjectRule::new(),
        ));
        registry.add(RewriteRule::MergeGetVerticesAndDedup(
            merge::MergeGetVerticesAndDedupRule::new(),
        ));
        registry.add(RewriteRule::MergeGetNbrsAndProject(
            merge::MergeGetNbrsAndProjectRule::new(),
        ));
        registry.add(RewriteRule::MergeGetNbrsAndDedup(
            merge::MergeGetNbrsAndDedupRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownTraverse(
            predicate_pushdown::PushFilterDownTraverseRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownExpandAll(
            predicate_pushdown::PushFilterDownExpandAllRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownNode(
            predicate_pushdown::PushFilterDownNodeRule::new(),
        ));
        registry.add(RewriteRule::PushEFilterDown(
            predicate_pushdown::PushEFilterDownRule::new(),
        ));
        registry.add(RewriteRule::PushVFilterDownScanVertices(
            predicate_pushdown::PushVFilterDownScanVerticesRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownInnerJoin(
            predicate_pushdown::PushFilterDownInnerJoinRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownHashInnerJoin(
            predicate_pushdown::PushFilterDownHashInnerJoinRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownHashLeftJoin(
            predicate_pushdown::PushFilterDownHashLeftJoinRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownCrossJoin(
            predicate_pushdown::PushFilterDownCrossJoinRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownGetNbrs(
            predicate_pushdown::PushFilterDownGetNbrsRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownAllPaths(
            predicate_pushdown::PushFilterDownAllPathsRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownScanVertices(
            projection_pushdown::PushProjectDownScanVerticesRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownScanEdges(
            projection_pushdown::PushProjectDownScanEdgesRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownGetVertices(
            projection_pushdown::PushProjectDownGetVerticesRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownGetEdges(
            projection_pushdown::PushProjectDownGetEdgesRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownGetNeighbors(
            projection_pushdown::PushProjectDownGetNeighborsRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownEdgeIndexScan(
            projection_pushdown::PushProjectDownEdgeIndexScanRule::new(),
        ));
        registry.add(RewriteRule::PushLimitDownGetVertices(
            limit_pushdown::PushLimitDownGetVerticesRule::new(),
        ));
        registry.add(RewriteRule::PushLimitDownGetEdges(
            limit_pushdown::PushLimitDownGetEdgesRule::new(),
        ));
        registry.add(RewriteRule::PushLimitDownScanVertices(
            limit_pushdown::PushLimitDownScanVerticesRule::new(),
        ));
        registry.add(RewriteRule::PushLimitDownScanEdges(
            limit_pushdown::PushLimitDownScanEdgesRule::new(),
        ));
        registry.add(RewriteRule::PushLimitDownIndexScan(
            limit_pushdown::PushLimitDownIndexScanRule::new(),
        ));
        registry.add(RewriteRule::PushTopNDownIndexScan(
            limit_pushdown::PushTopNDownIndexScanRule::new(),
        ));
        registry.add(RewriteRule::ConvertSortLimitToTopN(
            limit_pushdown::ConvertSortLimitToTopNRule::new(),
        ));
        registry.add(RewriteRule::PushFilterDownAggregate(
            aggregate::PushFilterDownAggregateRule::new(),
        ));
        registry.add(RewriteRule::PushProjectDownJoin(
            join_optimization::PushProjectDownJoinRule::new(),
        ));
        registry.add(RewriteRule::LeftJoinToInnerJoin(
            join_optimization::LeftJoinToInnerJoinRule::new(),
        ));
        registry.add(RewriteRule::JoinConditionSimplify(
            join_optimization::JoinConditionSimplifyRule::new(),
        ));
        registry.add(RewriteRule::JoinToExpand(
            join_optimization::JoinToExpandRule::new(),
        ));
        registry.add(RewriteRule::JoinToAppendVertices(
            join_optimization::JoinToAppendVerticesRule::new(),
        ));
        registry.add(RewriteRule::MergeConsecutiveExpand(
            join_optimization::MergeConsecutiveExpandRule::new(),
        ));
        registry.add(RewriteRule::JoinElimination(
            join_optimization::JoinEliminationRule::new(),
        ));
        registry.add(RewriteRule::IndexJoinSelection(
            join_optimization::IndexJoinSelectionRule::new(),
        ));
        registry.add(RewriteRule::JoinReorder(
            join_optimization::JoinReorderRule::new(),
        ));
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_registry_default() {
        let registry = RuleRegistry::default();
        assert_eq!(registry.len(), 49);
    }

    #[test]
    fn test_rule_names() {
        let registry = RuleRegistry::default();
        for rule in registry.iter() {
            let name = rule.name();
            assert!(!name.is_empty());
            assert!(name.ends_with("Rule"));
        }
    }
}

//! Hint-driven DP solving for the join-order enumerator.

use std::sync::Arc;

use super::JoinOrderEnumerator;
use crate::planning::join_order::join_tree::{JoinHintError, JoinTree, JoinTreeNode, TreeNodeType};
use crate::planning::join_order::query_graph::QueryGraph;
use crate::planning::join_order::subplans_table::JoinOrderPlan;
use crate::planning::join_order::subquery_graph::SubqueryGraph;
use crate::planning::plan::logical::LogicalNodeEnum;

impl JoinOrderEnumerator {
    pub(super) fn solve_tree_node(
        &mut self,
        node: &JoinTreeNode,
        query_graph: &Arc<QueryGraph>,
    ) -> Result<(LogicalNodeEnum, SubqueryGraph, u64, u64), JoinHintError> {
        match node.node_type {
            TreeNodeType::NodeScan => {
                let var = node
                    .extra_info
                    .join_nodes
                    .first()
                    .cloned()
                    .unwrap_or_default();
                let pos = query_graph
                    .node_pos(&var)
                    .ok_or_else(|| JoinHintError::UnknownVariable(var.clone()))?;
                let sg = SubqueryGraph::single_node(query_graph, pos);
                let best = self
                    .context
                    .sub_plans_table
                    .get_best_plan(&sg)
                    .cloned()
                    .ok_or(JoinHintError::NoCommonIntersectNode)?;
                Ok((best.plan.clone(), sg, best.cost, best.cardinality))
            }
            TreeNodeType::RelScan => {
                let var = node
                    .extra_info
                    .join_nodes
                    .first()
                    .cloned()
                    .unwrap_or_default();
                let pos = query_graph
                    .rel_pos(&var)
                    .ok_or_else(|| JoinHintError::UnknownVariable(var.clone()))?;
                // A rel scan binds its endpoints in the hint view (see
                // `covered_subgraph`): prefer the DP entry covering the
                // rel plus its endpoints so parent joins can key on the
                // endpoint variables. Fall back to the rel-only entry
                // when no endpoint-covering plan was enumerated.
                let covered = JoinTree { root: node.clone() }.covered_subgraph(query_graph);
                let rel_only = SubqueryGraph::single_rel(query_graph, pos);
                let (sg, best) = self
                    .context
                    .sub_plans_table
                    .get_best_plan(&covered)
                    .cloned()
                    .map(|p| (covered.clone(), p))
                    .or_else(|| {
                        self.context
                            .sub_plans_table
                            .get_best_plan(&rel_only)
                            .cloned()
                            .map(|p| (rel_only.clone(), p))
                    })
                    .ok_or(JoinHintError::NoCommonIntersectNode)?;
                Ok((best.plan.clone(), sg, best.cost, best.cardinality))
            }
            TreeNodeType::BinaryJoin => {
                if node.children.len() != 2 {
                    return Err(JoinHintError::NoCommonIntersectNode);
                }
                let (left_plan, left_sg, _, _) =
                    self.solve_tree_node(&node.children[0], query_graph)?;
                let (right_plan, right_sg, _, _) =
                    self.solve_tree_node(&node.children[1], query_graph)?;
                let left = self
                    .context
                    .sub_plans_table
                    .get_best_plan(&left_sg)
                    .cloned()
                    .map(|p| {
                        JoinOrderPlan::new(left_plan.clone(), p.cost, p.cardinality, p.encoding)
                    })
                    .unwrap_or_else(|| {
                        JoinOrderPlan::new(
                            left_plan.clone(),
                            0,
                            self.context.cardinality_estimator.estimate_scan(),
                            0,
                        )
                    });
                let right = self
                    .context
                    .sub_plans_table
                    .get_best_plan(&right_sg)
                    .cloned()
                    .map(|p| {
                        JoinOrderPlan::new(right_plan.clone(), p.cost, p.cardinality, p.encoding)
                    })
                    .unwrap_or_else(|| {
                        JoinOrderPlan::new(
                            right_plan.clone(),
                            0,
                            self.context.cardinality_estimator.estimate_scan(),
                            0,
                        )
                    });
                if !left_sg.is_joinable_with(&right_sg) {
                    return Err(JoinHintError::NoCommonIntersectNode);
                }
                let (plan, cost, card) =
                    self.append_inner_hash_join(&left, &right, &left_sg, &right_sg, query_graph);
                let mut combined = left_sg.clone();
                combined.add_subquery_graph(&right_sg);
                Ok((plan, combined, cost, card))
            }
            TreeNodeType::MultiwayJoin => {
                if node.children.is_empty() {
                    return Err(JoinHintError::MissingBuildSide);
                }
                let intersect_var = node
                    .extra_info
                    .join_nodes
                    .first()
                    .cloned()
                    .ok_or(JoinHintError::NoCommonIntersectNode)?;
                let (probe_plan, probe_sg, _, _) =
                    self.solve_tree_node(&node.children[0], query_graph)?;
                let mut build_plans = Vec::with_capacity(node.children.len() - 1);
                let mut bound_vars = Vec::with_capacity(node.children.len() - 1);
                let mut combined = probe_sg.clone();
                let probe_best = self
                    .context
                    .sub_plans_table
                    .get_best_plan(&probe_sg)
                    .cloned()
                    .map(|p| {
                        JoinOrderPlan::new(probe_plan.clone(), p.cost, p.cardinality, p.encoding)
                    })
                    .unwrap_or_else(|| {
                        JoinOrderPlan::new(
                            probe_plan.clone(),
                            0,
                            self.context.cardinality_estimator.estimate_scan(),
                            0,
                        )
                    });
                for child in &node.children[1..] {
                    let (build_plan, build_sg, _, _) = self.solve_tree_node(child, query_graph)?;
                    let probe_covers = |node_pos: usize| {
                        if probe_sg.contains_node(node_pos) {
                            return true;
                        }
                        for rel_pos in probe_sg.rel_positions() {
                            if let Some((src, dst)) = query_graph.rel_endpoint_positions(rel_pos) {
                                if src == node_pos || dst == node_pos {
                                    return true;
                                }
                            }
                        }
                        false
                    };
                    let mut bound_pos = build_sg
                        .node_positions()
                        .into_iter()
                        .find(|p| probe_covers(*p));
                    if bound_pos.is_none() {
                        for rel_pos in build_sg.rel_positions() {
                            if let Some((src, dst)) = query_graph.rel_endpoint_positions(rel_pos) {
                                if probe_covers(src) {
                                    bound_pos = Some(src);
                                    break;
                                }
                                if probe_covers(dst) {
                                    bound_pos = Some(dst);
                                    break;
                                }
                            }
                        }
                    }
                    let Some(bound_pos) = bound_pos else {
                        return Err(JoinHintError::NoCommonIntersectNode);
                    };
                    let bound_var = query_graph.query_nodes[bound_pos].variable.clone();
                    bound_vars.push(bound_var);
                    let build_best = self
                        .context
                        .sub_plans_table
                        .get_best_plan(&build_sg)
                        .cloned()
                        .map(|p| {
                            JoinOrderPlan::new(
                                build_plan.clone(),
                                p.cost,
                                p.cardinality,
                                p.encoding,
                            )
                        })
                        .unwrap_or_else(|| {
                            JoinOrderPlan::new(
                                build_plan.clone(),
                                0,
                                self.context.cardinality_estimator.estimate_scan(),
                                0,
                            )
                        });
                    build_plans.push(build_best);
                    combined.add_subquery_graph(&build_sg);
                }
                if probe_best
                    .plan
                    .col_names()
                    .iter()
                    .any(|c| c == &intersect_var)
                {
                    return Err(JoinHintError::NoCommonIntersectNode);
                }
                let Some((plan, cost, card)) = self.append_intersect(
                    &intersect_var,
                    &bound_vars,
                    &probe_best,
                    &build_plans,
                    query_graph,
                ) else {
                    return Err(JoinHintError::NoCommonIntersectNode);
                };
                if let Some(pos) = query_graph.node_pos(&intersect_var) {
                    combined.add_query_node(pos);
                }
                Ok((plan, combined, cost, card))
            }
        }
    }
}

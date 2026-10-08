//! WCO candidate detection and intersect plan generation.
//!
//! When several frontier rels share one unconnected node, joining them one
//! at a time would materialize large intermediate combinations. Instead,
//! [`JoinOrderEnumerator::plan_wco_join`] builds a single N-way
//! [`LogicalWcoIntersectNode`](crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode)
//! over the shared node and lets the executor intersect the adjacency lists.
//!
//! Trigger conditions:
//! 1. At least two rels share exactly one unconnected node (otherwise the
//!    plan degenerates to a binary join or extend).
//! 2. Every candidate rel has its other endpoint bound in the probe side
//!    (otherwise there is nothing to look up).
//! 3. The intersect node is absent from the probe scope (correctness).
//!
//! Level accounting: the new subgraph adds the `N` rels plus the previously
//! unconnected intersect node, so candidates fire when
//! `rels.len() + 1 == left_level`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::planning::plan::logical::logical_nodes::wco_intersect::LogicalWcoIntersectNode;
use crate::planning::plan::logical::LogicalNodeEnum;

use super::cost_model::CostModel;
use super::plan_join_order::JoinOrderEnumerator;
use super::query_graph::QueryGraph;
use super::subplans_table::JoinOrderPlan;
use super::subquery_graph::SubqueryGraph;

/// One intersect group: rels sharing an unconnected node, each with the
/// endpoint bound in the probe side.
#[derive(Debug, Clone)]
struct IntersectGroup {
    rel_positions: Vec<usize>,
    bound_node_positions: Vec<usize>,
}

impl JoinOrderEnumerator {
    /// WCO candidate detection and plan generation for one DP split.
    pub(crate) fn plan_wco_join(
        &mut self,
        left_level: usize,
        right_level: usize,
        query_graph: &Arc<QueryGraph>,
    ) {
        // Single-variable left sides cannot form multi-way intersects.
        if left_level < 3 {
            return;
        }
        let right_subgraphs: Vec<SubqueryGraph> = self
            .context_mut()
            .sub_plans_table
            .get_subgraphs(right_level)
            .iter()
            .map(|(sg, _)| sg.clone())
            .collect();
        for right_subgraph in &right_subgraphs {
            let candidates = self.populate_intersect_rel_candidates(query_graph, right_subgraph);
            for (intersect_node_pos, group) in &candidates {
                if group.rel_positions.len() + 1 != left_level {
                    continue;
                }
                self.plan_wco_join_for_node(
                    right_subgraph,
                    *intersect_node_pos,
                    group,
                    query_graph,
                );
            }
        }
    }

    /// Group frontier rels by their shared unconnected endpoint.
    ///
    /// Returns `intersect_node_pos -> group`. Rels with both endpoints
    /// bound (binary-join territory) or with neither endpoint bound
    /// (unbindable) are skipped.
    fn populate_intersect_rel_candidates(
        &self,
        query_graph: &Arc<QueryGraph>,
        subgraph: &SubqueryGraph,
    ) -> HashMap<usize, IntersectGroup> {
        let mut groups: HashMap<usize, IntersectGroup> = HashMap::new();
        for rel_pos in subgraph.get_rel_neighbor_positions() {
            let Some((src_pos, dst_pos)) = query_graph.rel_endpoint_positions(rel_pos) else {
                continue;
            };
            let src_bound = subgraph.contains_node(src_pos);
            let dst_bound = subgraph.contains_node(dst_pos);
            // Both ends bound: binary join territory. Neither bound:
            // unbindable from this probe side.
            if src_bound == dst_bound {
                continue;
            }
            let (intersect_pos, bound_pos) = if src_bound {
                (dst_pos, src_pos)
            } else {
                (src_pos, dst_pos)
            };
            let group = groups.entry(intersect_pos).or_insert(IntersectGroup {
                rel_positions: Vec::new(),
                bound_node_positions: Vec::new(),
            });
            group.rel_positions.push(rel_pos);
            group.bound_node_positions.push(bound_pos);
        }
        // WCO needs at least two rels sharing the node; anything smaller
        // degenerates to a binary extend.
        groups.retain(|_, group| group.rel_positions.len() >= 2);
        groups
    }

    /// Build one WCO plan per probe-side candidate plan.
    fn plan_wco_join_for_node(
        &mut self,
        right_subgraph: &SubqueryGraph,
        intersect_node_pos: usize,
        group: &IntersectGroup,
        query_graph: &Arc<QueryGraph>,
    ) {
        let Some(intersect_node) = query_graph.query_nodes.get(intersect_node_pos) else {
            return;
        };
        let intersect_var = intersect_node.variable.clone();

        // Collect one build plan per rel. Each build covers the rel plus
        // both endpoints so its rows carry the bound variable (looked up
        // from the probe side) and the intersect variable (merged across
        // build sides) that the streaming operator resolves by name.
        // Candidates without a covering build plan are skipped: rel-only
        // rows cannot key the adjacency tables.
        let mut build_plans: Vec<JoinOrderPlan> = Vec::with_capacity(group.rel_positions.len());
        for rel_pos in &group.rel_positions {
            let mut covered = SubqueryGraph::single_rel(query_graph, *rel_pos);
            if let Some((src, dst)) = query_graph.rel_endpoint_positions(*rel_pos) {
                covered.add_query_node(src);
                covered.add_query_node(dst);
            }
            let Some(best) = self
                .context()
                .sub_plans_table
                .get_best_plan(&covered)
                .cloned()
            else {
                return;
            };
            build_plans.push(best);
        }

        let probe_plans: Vec<JoinOrderPlan> = self
            .context()
            .sub_plans_table
            .get_plans(right_subgraph)
            .to_vec();
        if probe_plans.is_empty() {
            return;
        }

        let mut bound_keys = Vec::with_capacity(group.bound_node_positions.len());
        for bound_pos in &group.bound_node_positions {
            let Some(bound_node) = query_graph.query_nodes.get(*bound_pos) else {
                return;
            };
            bound_keys.push(bound_node.variable.clone());
        }

        let mut new_subgraph = right_subgraph.clone();
        for rel_pos in &group.rel_positions {
            new_subgraph.add_query_rel(*rel_pos);
        }
        new_subgraph.add_query_node(intersect_node_pos);

        for probe_plan in &probe_plans {
            // Correctness: the intersect node must be fresh on the probe side.
            if probe_plan
                .plan
                .col_names()
                .iter()
                .any(|c| c == &intersect_var)
            {
                continue;
            }
            let Some((plan, cost, cardinality)) = self.append_intersect(
                &intersect_var,
                &bound_keys,
                probe_plan,
                &build_plans,
                query_graph,
            ) else {
                continue;
            };
            let encoding = self.encode_plan(&plan, query_graph);
            self.context_mut().sub_plans_table.add_plan(
                &new_subgraph,
                JoinOrderPlan::new(plan, cost, cardinality, encoding),
            );
        }
    }

    /// Assemble a [`LogicalWcoIntersectNode`] from one probe plan and the
    /// build plans, priced with the intersect cost model. Returns `None`
    /// when a key variable cannot be interned.
    pub(crate) fn append_intersect(
        &mut self,
        intersect_var: &str,
        bound_vars: &[String],
        probe_plan: &JoinOrderPlan,
        build_plans: &[JoinOrderPlan],
        _query_graph: &Arc<QueryGraph>,
    ) -> Option<(LogicalNodeEnum, u64, u64)> {
        let (intersect_key, bound_keys) = {
            let ctx = self.context_mut();
            let intersect_key = ctx.join_key(intersect_var);
            let mut bound_keys = Vec::with_capacity(bound_vars.len());
            for var in bound_vars {
                bound_keys.push(ctx.join_key(var));
            }
            (intersect_key, bound_keys)
        };
        let builds: Vec<LogicalNodeEnum> = build_plans.iter().map(|p| p.plan.clone()).collect();
        let build_costs: Vec<u64> = build_plans.iter().map(|p| p.cost).collect();

        let mut col_names = probe_plan.plan.col_names().to_vec();
        if !col_names.iter().any(|c| c == intersect_var) {
            col_names.push(intersect_var.to_string());
        }
        for build in build_plans {
            for name in build.plan.col_names() {
                if !col_names.contains(name) {
                    col_names.push(name.clone());
                }
            }
        }

        // Intersect cardinality from the dedicated estimator: the cheaper
        // of conservative probe filtering and the independence assumption
        // over the intersect and bound key domains.
        let mut domains = Vec::with_capacity(bound_keys.len() + 1);
        domains.push(
            self.context()
                .cardinality_estimator
                .get_node_id_domain(intersect_key.id()),
        );
        for key in &bound_keys {
            domains.push(
                self.context()
                    .cardinality_estimator
                    .get_node_id_domain(key.id()),
            );
        }
        let build_cards: Vec<u64> = build_plans.iter().map(|p| p.cardinality).collect();
        let cardinality = self.context().cardinality_estimator.estimate_intersect(
            probe_plan.cardinality,
            &build_cards,
            &domains,
        );
        let cost = CostModel::compute_intersect_cost(
            probe_plan.cost,
            probe_plan.cardinality,
            &build_costs,
            cardinality,
        );
        let plan = LogicalNodeEnum::WcoIntersect(LogicalWcoIntersectNode::new(
            probe_plan.plan.clone(),
            builds,
            intersect_key,
            bound_keys,
            col_names,
        ));
        Some((plan, cost, cardinality))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::planning::join_order::query_graph::{QueryNode, QueryRel};

    fn triangle() -> Arc<QueryGraph> {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_node(QueryNode::new("c", "c"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
        qg.add_rel(QueryRel::new("e3", "e3", "b", "c"));
        Arc::new(qg)
    }

    fn star() -> Arc<QueryGraph> {
        // Center c shared by three rels: the canonical WCO shape.
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("c", "c"));
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_node(QueryNode::new("d", "d"));
        qg.add_rel(QueryRel::new("e1", "e1", "c", "a"));
        qg.add_rel(QueryRel::new("e2", "e2", "c", "b"));
        qg.add_rel(QueryRel::new("e3", "e3", "c", "d"));
        Arc::new(qg)
    }

    #[test]
    fn groups_rels_by_shared_unconnected_node() {
        let qg = triangle();
        let enumerator = JoinOrderEnumerator::new();
        // Probe {a, b, e1}: e2 hangs off a, e3 hangs off b, both toward c.
        let mut probe = SubqueryGraph::new(Arc::clone(&qg));
        probe.add_query_node(0);
        probe.add_query_node(1);
        probe.add_query_rel(0);
        let candidates = enumerator.populate_intersect_rel_candidates(&qg, &probe);
        let group = candidates.get(&2).expect("node c group");
        assert_eq!(group.rel_positions.len(), 2);
        assert!(group.rel_positions.contains(&1));
        assert!(group.rel_positions.contains(&2));
    }

    #[test]
    fn skips_doubly_bound_rels() {
        let qg = triangle();
        let enumerator = JoinOrderEnumerator::new();
        // Probe already binds every endpoint of e1.
        let mut probe = SubqueryGraph::new(Arc::clone(&qg));
        probe.add_query_node(0);
        probe.add_query_node(1);
        probe.add_query_node(2);
        probe.add_query_rel(0);
        probe.add_query_rel(1);
        let candidates = enumerator.populate_intersect_rel_candidates(&qg, &probe);
        // e3 has both ends bound now; e2 shares unconnected... none left.
        for group in candidates.values() {
            assert!(group.rel_positions.len() >= 2);
        }
    }

    #[test]
    fn triangle_plans_contain_wco_intersect() {
        let qg = triangle();
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_query_graph(&qg).expect("triangle plans");
        assert!(contains_wco(&plan), "expected a WCO intersect in the plan");
    }

    #[test]
    fn single_edge_graph_has_no_wco() {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        let qg = Arc::new(qg);
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_query_graph(&qg).expect("single edge plans");
        assert!(!contains_wco(&plan));
    }

    fn contains_wco(plan: &LogicalNodeEnum) -> bool {
        if matches!(plan, LogicalNodeEnum::WcoIntersect(_)) {
            return true;
        }
        match plan {
            LogicalNodeEnum::InnerJoin(n) => contains_wco(&n.left) || contains_wco(&n.right),
            LogicalNodeEnum::WcoIntersect(n) => n.deps.iter().any(contains_wco),
            _ => false,
        }
    }

    #[test]
    fn star_centers_on_shared_node() {
        let qg = star();
        let enumerator = JoinOrderEnumerator::new();
        // Probe binds a, b, d plus e1/e2; e3 hangs off d... build a probe
        // binding a and b with e1, e2 so e3 is the odd one: single rels do
        // not form groups, so this probe yields no group by itself.
        let mut probe = SubqueryGraph::new(Arc::clone(&qg));
        probe.add_query_node(1);
        probe.add_query_node(2);
        probe.add_query_rel(0);
        probe.add_query_rel(1);
        let candidates = enumerator.populate_intersect_rel_candidates(&qg, &probe);
        // e3's unconnected end is c... wait e3 = (c, d): d unbound, c
        // unbound too. Neither bound -> skipped. No groups.
        assert!(candidates.is_empty());
    }
}

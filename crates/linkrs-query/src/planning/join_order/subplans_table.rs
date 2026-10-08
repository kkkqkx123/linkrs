//! DP table of candidate subplans.
//!
//! Each connected subgraph keeps up to [`SubgraphPlans::MAX_NUM_PLANS`]
//! candidate plans indexed by their factorization encoding (which node groups
//! are flat). Keeping several encodings per subgraph matters because whether
//! a variable is flat or unflat changes downstream flatten costs.

use std::collections::HashMap;

use crate::planning::plan::logical::LogicalNodeEnum;

use super::subquery_graph::SubqueryGraph;

/// A single candidate plan with its estimated cost and cardinality.
#[derive(Debug, Clone)]
pub struct JoinOrderPlan {
    pub plan: LogicalNodeEnum,
    pub cost: u64,
    pub cardinality: u64,
    /// Factorization encoding: bit `i` set when the `i`-th tracked node is
    /// flat in this plan.
    pub encoding: u64,
}

impl JoinOrderPlan {
    pub fn new(plan: LogicalNodeEnum, cost: u64, cardinality: u64, encoding: u64) -> Self {
        Self {
            plan,
            cost,
            cardinality,
            encoding,
        }
    }
}

/// Candidate plans for one subgraph keyed by factorization encoding.
#[derive(Debug, Clone, Default)]
pub struct SubgraphPlans {
    /// Cheapest cost seen so far; used for pruning.
    pub max_cost: u64,
    /// Factorization encoding -> index into [`SubgraphPlans::plans`].
    pub encoded_plan_to_idx: HashMap<u64, usize>,
    /// Candidate plans for this subgraph.
    pub plans: Vec<JoinOrderPlan>,
}

impl SubgraphPlans {
    /// Maximum number of plans retained per subgraph.
    pub const MAX_NUM_PLANS: usize = 10;

    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a plan, keeping the cheapest plan per encoding and capping the
    /// total at [`SubgraphPlans::MAX_NUM_PLANS`] by dropping the most
    /// expensive candidate.
    pub fn add_plan(&mut self, plan: JoinOrderPlan) {
        if let Some(idx) = self.encoded_plan_to_idx.get(&plan.encoding).copied() {
            if plan.cost < self.plans[idx].cost {
                self.plans[idx] = plan;
            }
            self.refresh_max_cost();
            return;
        }
        if self.plans.len() >= Self::MAX_NUM_PLANS {
            if plan.cost >= self.max_cost {
                return;
            }
            if let Some(pos) = self.plans.iter().position(|p| p.cost == self.max_cost) {
                let removed = self.plans.remove(pos);
                self.encoded_plan_to_idx.remove(&removed.encoding);
                for idx in self.encoded_plan_to_idx.values_mut() {
                    if *idx > pos {
                        *idx -= 1;
                    }
                }
            }
        }
        self.encoded_plan_to_idx
            .insert(plan.encoding, self.plans.len());
        if plan.cost > self.max_cost {
            self.max_cost = plan.cost;
        }
        self.plans.push(plan);
    }

    /// Cheapest plan for this subgraph.
    pub fn best_plan(&self) -> Option<&JoinOrderPlan> {
        self.plans.iter().min_by_key(|p| p.cost)
    }

    fn refresh_max_cost(&mut self) {
        self.max_cost = self.plans.iter().map(|p| p.cost).max().unwrap_or(0);
    }
}

/// DP table organized by subgraph size (total variables).
///
/// `dp_levels[n]` holds every enumerated subgraph with exactly `n`
/// variables (nodes + rels) plus its candidate plans.
#[derive(Debug, Default)]
pub struct SubPlansTable {
    pub dp_levels: Vec<Vec<(SubqueryGraph, SubgraphPlans)>>,
}

impl SubPlansTable {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_level(&mut self, level: usize) {
        while self.dp_levels.len() <= level {
            self.dp_levels.push(Vec::new());
        }
    }

    fn find_entry_mut(&mut self, subgraph: &SubqueryGraph) -> Option<&mut SubgraphPlans> {
        let level = subgraph.total_num_variables();
        let key = subgraph.key();
        self.dp_levels
            .get_mut(level)?
            .iter_mut()
            .find_map(
                |(sg, plans)| {
                    if sg.key() == key {
                        Some(plans)
                    } else {
                        None
                    }
                },
            )
    }

    /// Insert a candidate plan for the given subgraph.
    pub fn add_plan(&mut self, subgraph: &SubqueryGraph, plan: JoinOrderPlan) {
        let level = subgraph.total_num_variables();
        self.ensure_level(level);
        match self.find_entry_mut(subgraph) {
            Some(entry) => entry.add_plan(plan),
            None => {
                let mut plans = SubgraphPlans::new();
                plans.add_plan(plan);
                self.dp_levels[level].push((subgraph.clone(), plans));
            }
        }
    }

    /// Cheapest plan for an exact subgraph match.
    pub fn get_best_plan(&self, subgraph: &SubqueryGraph) -> Option<&JoinOrderPlan> {
        let level = subgraph.total_num_variables();
        let key = subgraph.key();
        self.dp_levels.get(level)?.iter().find_map(|(sg, plans)| {
            if sg.key() == key {
                plans.best_plan()
            } else {
                None
            }
        })
    }

    /// All candidate plans for an exact subgraph match.
    pub fn get_plans(&self, subgraph: &SubqueryGraph) -> &[JoinOrderPlan] {
        let level = subgraph.total_num_variables();
        let key = subgraph.key();
        self.dp_levels
            .get(level)
            .and_then(|entries| {
                entries.iter().find_map(|(sg, plans)| {
                    if sg.key() == key {
                        Some(plans.plans.as_slice())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(&[])
    }

    /// All enumerated subgraphs with exactly `level` variables.
    pub fn get_subgraphs(&self, level: usize) -> &[(SubqueryGraph, SubgraphPlans)] {
        self.dp_levels.get(level).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Number of DP levels currently stored.
    pub fn num_levels(&self) -> usize {
        self.dp_levels.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::planning::join_order::query_graph::QueryGraph;
    use crate::planning::join_order::query_graph::{QueryNode, QueryRel};
    use crate::planning::plan::core::next_node_id;
    use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;

    fn scan_node(var: &str) -> LogicalNodeEnum {
        LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 1,
            space_name: "default".to_string(),
            tag: None,
            expression: None,
            limit: None,
            projected_properties: vec![],
            index_hint: None,
            estimated_cardinality: None,
            output_var: Some(var.to_string()),
            col_names: vec![var.to_string()],
            column_types: vec![],
        })
    }

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

    #[test]
    fn add_and_best_plan() {
        let qg = triangle();
        let mut table = SubPlansTable::new();
        let sg = SubqueryGraph::single_node(&qg, 0);
        table.add_plan(&sg, JoinOrderPlan::new(scan_node("a"), 100, 1_000, 0b1));
        table.add_plan(&sg, JoinOrderPlan::new(scan_node("a"), 50, 1_000, 0b1));
        let best = table.get_best_plan(&sg).expect("best plan");
        assert_eq!(best.cost, 50);
    }

    #[test]
    fn distinct_encodings_coexist() {
        let qg = triangle();
        let mut table = SubPlansTable::new();
        let sg = SubqueryGraph::single_node(&qg, 0);
        table.add_plan(&sg, JoinOrderPlan::new(scan_node("a"), 100, 1_000, 0b01));
        table.add_plan(&sg, JoinOrderPlan::new(scan_node("a"), 200, 1_000, 0b10));
        assert_eq!(table.get_plans(&sg).len(), 2);
    }

    #[test]
    fn plan_cap_drops_most_expensive() {
        let qg = triangle();
        let mut table = SubPlansTable::new();
        let sg = SubqueryGraph::single_node(&qg, 0);
        for i in 0..(SubgraphPlans::MAX_NUM_PLANS as u64 + 2) {
            table.add_plan(&sg, JoinOrderPlan::new(scan_node("a"), 10 + i, 1_000, i));
        }
        assert_eq!(table.get_plans(&sg).len(), SubgraphPlans::MAX_NUM_PLANS);
        let best = table.get_best_plan(&sg).expect("best plan");
        assert_eq!(best.cost, 10);
    }

    #[test]
    fn missing_subgraph_returns_none() {
        let qg = triangle();
        let table = SubPlansTable::new();
        let sg = SubqueryGraph::single_node(&qg, 1);
        assert!(table.get_best_plan(&sg).is_none());
        assert!(table.get_plans(&sg).is_empty());
        assert!(table.get_subgraphs(1).is_empty());
    }
}

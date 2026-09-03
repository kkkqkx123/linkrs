//! DP join order enumeration framework.
//!
//! [`JoinOrderEnumerator`] prices base table scans, then fills the DP table
//! level by level from small subgraphs to the full query graph. Exact
//! enumeration tries every `(left, right)` split up to
//! [`MAX_LEVEL_TO_PLAN_EXACTLY`]; larger levels only try `leftLevel == 1`
//! splits to avoid the combinatorial explosion. Binary joins live here; the
//! WCO intersect hook (`plan_wco_join`, Phase 3) plugs into the exact path.

use std::collections::HashMap;
use std::sync::Arc;

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::Expression;

use crate::planning::plan::core::next_node_id;
use crate::planning::plan::logical::logical_nodes::access::{
    LogicalScanEdgesNode, LogicalScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::join::LogicalInnerJoinNode;
use crate::planning::plan::logical::LogicalNodeEnum;

use super::cardinality_estimator::{CardinalityEstimator, JoinOrderStats};
use super::cost_model::CostModel;
use super::join_tree::{JoinHint, JoinHintError, JoinTreeConstructor, JoinTreeNode, TreeNodeType};
use super::query_graph::QueryGraph;
use super::subplans_table::{JoinOrderPlan, SubPlansTable};
use super::subquery_graph::SubqueryGraph;

/// Exact enumeration is used up to this level; larger levels use the
/// approximate (left-deep with level-1 right side) strategy.
pub const MAX_LEVEL_TO_PLAN_EXACTLY: usize = 7;

/// Mutable DP state threaded through enumeration.
pub struct JoinOrderEnumeratorContext {
    pub sub_plans_table: SubPlansTable,
    pub cardinality_estimator: CardinalityEstimator,
    pub cost_model: CostModel,
    pub max_cost: u64,
    space_id: u64,
    space_name: String,
    expr_ctx: Arc<ExpressionAnalysisContext>,
    key_cache: HashMap<String, ContextualExpression>,
}

impl std::fmt::Debug for JoinOrderEnumeratorContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinOrderEnumeratorContext")
            .field("max_cost", &self.max_cost)
            .field("num_dp_levels", &self.sub_plans_table.num_levels())
            .finish()
    }
}

impl Default for JoinOrderEnumeratorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl JoinOrderEnumeratorContext {
    pub fn new() -> Self {
        Self {
            sub_plans_table: SubPlansTable::new(),
            cardinality_estimator: CardinalityEstimator::new(),
            cost_model: CostModel,
            max_cost: u64::MAX,
            space_id: 1,
            space_name: "default".to_string(),
            expr_ctx: Arc::new(ExpressionAnalysisContext::new()),
            key_cache: HashMap::new(),
        }
    }

    /// Space the base scans belong to (mirrors the MATCH planning context).
    pub fn with_space(mut self, space_id: u64, space_name: impl Into<String>) -> Self {
        self.space_id = space_id;
        self.space_name = space_name.into();
        self
    }

    /// Attach real cardinality statistics to the estimator.
    pub fn with_stats(mut self, stats: JoinOrderStats) -> Self {
        self.cardinality_estimator.set_stats(stats);
        self
    }

    /// Intern a variable join key so both sides of a hash join share one
    /// expression identity.
    pub fn join_key(&mut self, variable: &str) -> ContextualExpression {
        if let Some(cached) = self.key_cache.get(variable) {
            return cached.clone();
        }
        let id = self
            .expr_ctx
            .register_expression(ExpressionMeta::new(Expression::Variable(
                variable.to_string(),
            )));
        let key = ContextualExpression::new(id, Arc::clone(&self.expr_ctx));
        self.key_cache.insert(variable.to_string(), key.clone());
        key
    }
}

/// DP join order enumerator.
#[derive(Debug, Default)]
pub struct JoinOrderEnumerator {
    context: JoinOrderEnumeratorContext,
}

impl JoinOrderEnumerator {
    pub fn new() -> Self {
        Self {
            context: JoinOrderEnumeratorContext::new(),
        }
    }

    /// Space the base scans belong to.
    pub fn with_space(mut self, space_id: u64, space_name: impl Into<String>) -> Self {
        self.context = std::mem::take(&mut self.context).with_space(space_id, space_name);
        self
    }

    /// Attach real cardinality statistics to the estimator.
    pub fn with_stats(mut self, stats: JoinOrderStats) -> Self {
        self.context = std::mem::take(&mut self.context).with_stats(stats);
        self
    }

    pub fn context(&self) -> &JoinOrderEnumeratorContext {
        &self.context
    }

    pub(crate) fn context_mut(&mut self) -> &mut JoinOrderEnumeratorContext {
        &mut self.context
    }

    /// Seed base scans and fill the DP table level by level, so both
    /// automatic enumeration and hint solving reuse fully priced entries.
    fn enumerate_levels(&mut self, query_graph: &Arc<QueryGraph>) {
        self.plan_base_table_scans(query_graph);
        let max_level = query_graph.num_nodes() + query_graph.num_rels();
        for level in 2..=max_level {
            if level <= MAX_LEVEL_TO_PLAN_EXACTLY {
                self.plan_level_exactly(level, query_graph);
            } else {
                self.plan_level_approximately(level, query_graph);
            }
        }
    }

    /// Enumerate the join order for a query graph and return the cheapest
    /// plan covering every node and rel, if one exists.
    pub fn plan_query_graph(&mut self, query_graph: &Arc<QueryGraph>) -> Option<LogicalNodeEnum> {
        self.enumerate_levels(query_graph);
        let mut full = SubqueryGraph::new(Arc::clone(query_graph));
        for node_pos in 0..query_graph.num_nodes() {
            full.add_query_node(node_pos);
        }
        for rel_pos in 0..query_graph.num_rels() {
            full.add_query_rel(rel_pos);
        }
        self.context
            .sub_plans_table
            .get_best_plan(&full)
            .map(|p| p.plan.clone())
    }

    /// Seed level-1 plans: one node scan per query node, one rel scan per
    /// query rel.
    ///
    /// Base scans carry the interned variable as `expression` so their
    /// factorized schemas are non-empty and downstream `encode_plan`
    /// can distinguish flat/unflat variants.
    fn plan_base_table_scans(&mut self, query_graph: &Arc<QueryGraph>) {
        for (node_pos, node) in query_graph.query_nodes.iter().enumerate() {
            let key = self.context.join_key(&node.variable);
            let cardinality = self
                .context
                .cardinality_estimator
                .estimate_node_scan(&node.labels);
            self.context
                .cardinality_estimator
                .set_node_id_domain(key.id().clone(), cardinality);
            let plan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
                id: next_node_id(),
                space_id: self.context.space_id,
                space_name: self.context.space_name.clone(),
                tag: node.labels.first().cloned(),
                expression: Some(key),
                limit: None,
                projected_properties: vec![],
                index_hint: None,
                estimated_cardinality: Some(cardinality),
                output_var: Some(node.variable.clone()),
                col_names: vec![node.variable.clone()],
                column_types: vec![],
            });
            let cost = CostModel::compute_extend_cost(0, cardinality);
            let encoding = self.encode_plan(&plan, query_graph);
            let subgraph = SubqueryGraph::single_node(query_graph, node_pos);
            self.context.sub_plans_table.add_plan(
                &subgraph,
                JoinOrderPlan::new(plan, cost, cardinality, encoding),
            );
        }
        for (rel_pos, rel) in query_graph.query_rels.iter().enumerate() {
            let key = self.context.join_key(&rel.variable);
            let cardinality = self
                .context
                .cardinality_estimator
                .estimate_rel_scan(&rel.edge_types);
            let plan = LogicalNodeEnum::ScanEdges(LogicalScanEdgesNode {
                id: next_node_id(),
                space_id: self.context.space_id,
                edge_type: rel.edge_types.first().cloned(),
                expression: Some(key),
                limit: None,
                projected_properties: vec![],
                index_hint: None,
                estimated_cardinality: Some(cardinality),
                output_var: Some(rel.variable.clone()),
                col_names: vec![rel.variable.clone()],
                column_types: vec![],
            });
            let cost = CostModel::compute_extend_cost(0, cardinality);
            let encoding = self.encode_plan(&plan, query_graph);
            let subgraph = SubqueryGraph::single_rel(query_graph, rel_pos);
            self.context.sub_plans_table.add_plan(
                &subgraph,
                JoinOrderPlan::new(plan, cost, cardinality, encoding),
            );
        }
    }

    /// Encode the factorization structure of a candidate plan as a bitset.
    ///
    /// Bit `i` is set when the `i`-th query node is flat in the plan schema.
    /// Plans with different encodings are kept as separate DP candidates
    /// because flatness changes downstream flatten costs.
    pub(crate) fn encode_plan(
        &mut self,
        plan: &LogicalNodeEnum,
        query_graph: &Arc<QueryGraph>,
    ) -> u64 {
        let mut owned = plan.clone();
        let schema = Self::compute_schema_for_plan(&mut owned);
        let mut encoding = 0u64;
        for (i, node) in query_graph.query_nodes.iter().enumerate().take(64) {
            let key = self.context.join_key(&node.variable);
            let id = key.id().clone();
            if let Some(pos) = schema.get_group_pos(&id) {
                if let Some(group) = schema.get_group(pos) {
                    if group.is_flat() {
                        encoding |= 1u64 << i;
                    }
                }
            } else if schema
                .get_group_pos_by_name_opt(&node.variable)
                .and_then(|pos| schema.get_group(pos))
                .is_some_and(|g| g.is_flat())
            {
                encoding |= 1u64 << i;
            }
        }
        encoding
    }

    fn compute_schema_for_plan(
        plan: &mut LogicalNodeEnum,
    ) -> crate::planning::plan::factorization::FactorizedSchema {
        use crate::planning::plan::factorization::FactorizedSchemaCompute;
        match plan {
            LogicalNodeEnum::InnerJoin(n) => {
                let mut left = (*n.left).clone();
                let mut right = (*n.right).clone();
                let left_schema = Self::compute_schema_for_plan(&mut left);
                let right_schema = Self::compute_schema_for_plan(&mut right);
                plan.compute_factorized_schema(&[left_schema, right_schema])
            }
            LogicalNodeEnum::WcoIntersect(n) => {
                let mut child_schemas = Vec::with_capacity(n.deps.len());
                for dep in n.deps.iter().cloned() {
                    let mut owned = dep;
                    child_schemas.push(Self::compute_schema_for_plan(&mut owned));
                }
                plan.compute_factorized_schema(&child_schemas)
            }
            _ => plan.compute_factorized_schema(&[]),
        }
    }

    /// Exact enumeration: try every `(left_level, right_level)` split.
    fn plan_level_exactly(&mut self, level: usize, query_graph: &Arc<QueryGraph>) {
        let max_left = level / 2;
        for left_level in 1..=max_left.max(1).min(level - 1) {
            let right_level = level - left_level;
            if left_level > 1 {
                self.plan_wco_join(left_level, right_level, query_graph);
            }
            self.plan_inner_join(left_level, right_level, query_graph);
        }
    }

    /// Approximate enumeration: only `leftLevel == 1` splits.
    fn plan_level_approximately(&mut self, level: usize, query_graph: &Arc<QueryGraph>) {
        if level < 2 {
            return;
        }
        self.plan_inner_join(1, level - 1, query_graph);
    }

    /// Binary inner joins between adjacent left/right subgraphs.
    fn plan_inner_join(
        &mut self,
        left_level: usize,
        right_level: usize,
        query_graph: &Arc<QueryGraph>,
    ) {
        let right_subgraphs: Vec<SubqueryGraph> = self
            .context
            .sub_plans_table
            .get_subgraphs(right_level)
            .iter()
            .map(|(sg, _)| sg.clone())
            .collect();
        for right_subgraph in &right_subgraphs {
            let neighbors = right_subgraph.get_neighbor_subgraphs(left_level);
            for nbr_subgraph in &neighbors {
                if !nbr_subgraph.is_joinable_with(right_subgraph) {
                    continue;
                }
                // Prefer index-nested-loop extends for pure extend pairs;
                // fall back to hash joins on shared nodes.
                if self.try_plan_extend_join(right_subgraph, nbr_subgraph, query_graph) {
                    continue;
                }
                self.plan_inner_hash_join(right_subgraph, nbr_subgraph, query_graph);
            }
        }
    }

    /// Extend join for pairs without shared nodes but with a rel-to-node
    /// step in either direction. Returns false when the pair needs a hash
    /// join instead.
    fn try_plan_extend_join(
        &mut self,
        right_subgraph: &SubqueryGraph,
        nbr_subgraph: &SubqueryGraph,
        query_graph: &Arc<QueryGraph>,
    ) -> bool {
        if right_subgraph.is_connected(nbr_subgraph) {
            return false;
        }
        if !right_subgraph.is_extendable_with(nbr_subgraph) {
            return false;
        }
        let right_plans: Vec<JoinOrderPlan> = self
            .context
            .sub_plans_table
            .get_plans(right_subgraph)
            .to_vec();
        let nbr_plans: Vec<JoinOrderPlan> = self
            .context
            .sub_plans_table
            .get_plans(nbr_subgraph)
            .to_vec();
        if right_plans.is_empty() || nbr_plans.is_empty() {
            return false;
        }
        let mut combined = right_subgraph.clone();
        combined.add_subquery_graph(nbr_subgraph);
        for right_plan in &right_plans {
            for nbr_plan in &nbr_plans {
                let (plan, cost, cardinality) =
                    self.append_extend(right_plan, nbr_plan, query_graph);
                let encoding = self.encode_plan(&plan, query_graph);
                self.context.sub_plans_table.add_plan(
                    &combined,
                    JoinOrderPlan::new(plan, cost, cardinality, encoding),
                );
            }
        }
        true
    }

    /// Hash join on the shared node positions of two connected subgraphs.
    fn plan_inner_hash_join(
        &mut self,
        right_subgraph: &SubqueryGraph,
        nbr_subgraph: &SubqueryGraph,
        query_graph: &Arc<QueryGraph>,
    ) {
        if !right_subgraph.is_connected(nbr_subgraph) {
            return;
        }
        let right_plans: Vec<JoinOrderPlan> = self
            .context
            .sub_plans_table
            .get_plans(right_subgraph)
            .to_vec();
        let nbr_plans: Vec<JoinOrderPlan> = self
            .context
            .sub_plans_table
            .get_plans(nbr_subgraph)
            .to_vec();
        if right_plans.is_empty() || nbr_plans.is_empty() {
            return;
        }
        let mut combined = right_subgraph.clone();
        combined.add_subquery_graph(nbr_subgraph);
        for right_plan in &right_plans {
            for nbr_plan in &nbr_plans {
                let (plan, cost, cardinality) = self.append_inner_hash_join(
                    right_plan,
                    nbr_plan,
                    right_subgraph,
                    nbr_subgraph,
                    query_graph,
                );
                let encoding = self.encode_plan(&plan, query_graph);
                self.context.sub_plans_table.add_plan(
                    &combined,
                    JoinOrderPlan::new(plan, cost, cardinality, encoding),
                );
            }
        }
    }

    /// Build a hash join node with one key per shared node variable.
    fn append_inner_hash_join(
        &mut self,
        probe: &JoinOrderPlan,
        build: &JoinOrderPlan,
        probe_subgraph: &SubqueryGraph,
        build_subgraph: &SubqueryGraph,
        query_graph: &Arc<QueryGraph>,
    ) -> (LogicalNodeEnum, u64, u64) {
        let shared = probe_subgraph.get_connected_node_positions(build_subgraph);
        let mut hash_keys = Vec::new();
        let mut probe_keys = Vec::new();
        let mut domains = Vec::new();
        for node_pos in &shared {
            if let Some(node) = query_graph.query_nodes.get(*node_pos) {
                let key = self.context.join_key(&node.variable);
                domains.push(
                    self.context
                        .cardinality_estimator
                        .get_node_id_domain(key.id()),
                );
                hash_keys.push(key.clone());
                probe_keys.push(key);
            }
        }
        let cardinality = self.context.cardinality_estimator.estimate_hash_join(
            probe.cardinality,
            build.cardinality,
            &domains,
        );
        let join_key_card = domains.into_iter().min().unwrap_or(cardinality);
        // Charge the emitted rows on top of the probe/build scan price:
        // the executor materializes every output row (hash table plus
        // downstream rescans), and pricing scans alone lets deep binary
        // chains hide intermediate blowup. Without this, honestly priced
        // WCO intersects could never win automatic cost competition on
        // cyclic queries.
        let cost = CostModel::compute_hash_join_cost(
            probe.cost,
            probe.cardinality,
            build.cost,
            join_key_card,
        )
        .saturating_add(cardinality);
        let mut col_names = probe.plan.col_names().to_vec();
        for name in build.plan.col_names() {
            if !col_names.contains(name) {
                col_names.push(name.clone());
            }
        }
        let plan = LogicalNodeEnum::InnerJoin(LogicalInnerJoinNode {
            id: next_node_id(),
            left: Box::new(probe.plan.clone()),
            right: Box::new(build.plan.clone()),
            hash_keys,
            probe_keys,
            deps: vec![probe.plan.clone(), build.plan.clone()],
            output_var: None,
            col_names,
            column_types: vec![],
        });
        (plan, cost, cardinality)
    }

    /// Build an extend join node (empty keys: nested-loop expansion at
    /// execution time) priced as an extend of the probe side. The fanout
    /// uses the statistics average-degree hint capped by the build
    /// cardinality, so skewed real graphs price extends correctly.
    fn append_extend(
        &mut self,
        probe: &JoinOrderPlan,
        build: &JoinOrderPlan,
        _query_graph: &Arc<QueryGraph>,
    ) -> (LogicalNodeEnum, u64, u64) {
        let fanout = build
            .cardinality
            .min(self.context.cardinality_estimator.avg_degree_hint());
        let cardinality = self
            .context
            .cardinality_estimator
            .estimate_extend(probe.cardinality, fanout);
        // Charge the emitted rows as well: a keyless extend buffers the
        // build side and writes every fanned-out row, so pricing the probe
        // scan alone hides the blowup (e.g. a triangle extend chain emitting
        // millions of rows for a few hundred thousand cost units) and prices
        // WCO intersects out of automatic selection.
        let cost = CostModel::compute_extend_cost(probe.cost, probe.cardinality)
            .saturating_add(build.cost)
            .saturating_add(cardinality);
        let mut col_names = probe.plan.col_names().to_vec();
        for name in build.plan.col_names() {
            if !col_names.contains(name) {
                col_names.push(name.clone());
            }
        }
        let plan = LogicalNodeEnum::InnerJoin(LogicalInnerJoinNode {
            id: next_node_id(),
            left: Box::new(probe.plan.clone()),
            right: Box::new(build.plan.clone()),
            hash_keys: vec![],
            probe_keys: vec![],
            deps: vec![probe.plan.clone(), build.plan.clone()],
            output_var: None,
            col_names,
            column_types: vec![],
        });
        (plan, cost, cardinality)
    }

    /// Plan a query graph following a user join hint.
    ///
    /// The hint is resolved to a [`JoinTree`](super::join_tree::JoinTree)
    /// and then solved bottom-up against the enumerated DP table: leaf
    /// scans reuse the cheapest covering entry (a rel scan prefers the
    /// rel-plus-endpoints plan), binary nodes become hash joins on the
    /// shared nodes, and multi-way nodes become `WcoIntersect` via
    /// `append_intersect`. Hints normally arrive from `USING JOIN`
    /// clauses; see [`JoinHint::from_ast`].
    pub fn plan_with_hint(
        &mut self,
        query_graph: &Arc<QueryGraph>,
        hint: &JoinHint,
    ) -> Result<LogicalNodeEnum, JoinHintError> {
        // Enumerate first so hint leaves reuse fully priced DP entries
        // (e.g. a rel scan covering its endpoints for binary rel joins).
        self.enumerate_levels(query_graph);
        let tree = JoinTreeConstructor::construct(query_graph, hint)?;
        let (plan, _subgraph, _cost, _card) = self.solve_tree_node(&tree.root, query_graph)?;
        Ok(plan)
    }

    fn solve_tree_node(
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
                let covered =
                    super::join_tree::JoinTree { root: node.clone() }.covered_subgraph(query_graph);
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

    fn single_edge() -> Arc<QueryGraph> {
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        Arc::new(qg)
    }

    #[test]
    fn base_scans_seed_level_one() {
        let qg = triangle();
        let mut enumerator = JoinOrderEnumerator::new();
        enumerator.plan_base_table_scans(&qg);
        assert_eq!(enumerator.context.sub_plans_table.get_subgraphs(1).len(), 6);
    }

    #[test]
    fn single_edge_graph_plans_end_to_end() {
        let qg = single_edge();
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_query_graph(&qg);
        assert!(plan.is_some(), "single-edge graph should plan");
    }

    #[test]
    fn triangle_graph_plans_end_to_end() {
        let qg = triangle();
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_query_graph(&qg);
        assert!(plan.is_some(), "triangle graph should plan");
    }

    #[test]
    fn join_key_interning_is_stable() {
        let mut ctx = JoinOrderEnumeratorContext::new();
        let first = ctx.join_key("a");
        let second = ctx.join_key("a");
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn base_scan_encoding_marks_node_flat() {
        let qg = triangle();
        let mut enumerator = JoinOrderEnumerator::new();
        enumerator.plan_base_table_scans(&qg);
        let sg = crate::planning::join_order::SubqueryGraph::single_node(&qg, 0);
        let best = enumerator
            .context
            .sub_plans_table
            .get_best_plan(&sg)
            .expect("base plan");
        assert_ne!(best.encoding, 0, "base scan must carry flat encoding");
        assert_eq!(best.encoding & 1, 1);
    }

    #[test]
    fn wco_and_hash_encodings_differ_on_triangle() {
        let qg = triangle();
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_query_graph(&qg).expect("triangle plans");
        let encoding = enumerator.encode_plan(&plan, &qg);
        assert_ne!(encoding, 0, "triangle plan must carry non-zero encoding");
    }

    #[test]
    fn hint_binary_join_plans() {
        use crate::planning::join_order::{JoinHint, JoinHintNode};
        let qg = single_edge();
        let hint = JoinHint {
            root: JoinHintNode::Binary {
                left: Box::new(JoinHintNode::Scan {
                    variable: "a".to_string(),
                }),
                right: Box::new(JoinHintNode::Scan {
                    variable: "e1".to_string(),
                }),
            },
        };
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
        assert!(matches!(plan, LogicalNodeEnum::InnerJoin(_)));
    }

    #[test]
    fn stats_snapshot_drives_base_scan_cards() {
        use super::super::cardinality_estimator::JoinOrderStats;
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a").with_labels(vec!["person".to_string()]));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b").with_edge_types(vec!["knows".to_string()]));
        let qg = Arc::new(qg);
        let mut stats = JoinOrderStats::default();
        stats.vertex_counts.insert("person".to_string(), 5_000);
        stats.edge_counts.insert("knows".to_string(), 20_000);
        stats.avg_out_degrees.insert("knows".to_string(), 4.0);
        let mut enumerator = JoinOrderEnumerator::new().with_stats(stats);
        enumerator.plan_base_table_scans(&qg);
        let node_sg = crate::planning::join_order::SubqueryGraph::single_node(&qg, 0);
        let node_best = enumerator
            .context
            .sub_plans_table
            .get_best_plan(&node_sg)
            .expect("node plan");
        assert_eq!(node_best.cardinality, 5_000);
        let rel_sg = crate::planning::join_order::SubqueryGraph::single_rel(&qg, 0);
        let rel_best = enumerator
            .context
            .sub_plans_table
            .get_best_plan(&rel_sg)
            .expect("rel plan");
        assert_eq!(rel_best.cardinality, 20_000);
        assert_eq!(
            enumerator.context.cardinality_estimator.avg_degree_hint(),
            4
        );
    }

    #[test]
    fn hint_binary_rel_join_plans() {
        use crate::planning::join_order::{JoinHint, JoinHintNode};
        let mut qg = QueryGraph::new();
        qg.add_node(QueryNode::new("a", "a"));
        qg.add_node(QueryNode::new("b", "b"));
        qg.add_node(QueryNode::new("c", "c"));
        qg.add_rel(QueryRel::new("e1", "e1", "a", "b"));
        qg.add_rel(QueryRel::new("e2", "e2", "a", "c"));
        let qg = Arc::new(qg);
        let hint = JoinHint {
            root: JoinHintNode::Binary {
                left: Box::new(JoinHintNode::Scan {
                    variable: "e1".to_string(),
                }),
                right: Box::new(JoinHintNode::Scan {
                    variable: "e2".to_string(),
                }),
            },
        };
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
        assert!(matches!(plan, LogicalNodeEnum::InnerJoin(_)));
    }

    #[test]
    fn hint_multiway_join_plans_wco() {
        use crate::planning::join_order::{JoinHint, JoinHintNode};
        let qg = triangle();
        let hint = JoinHint {
            root: JoinHintNode::Multiway {
                probe: Box::new(JoinHintNode::Binary {
                    left: Box::new(JoinHintNode::Scan {
                        variable: "a".to_string(),
                    }),
                    right: Box::new(JoinHintNode::Scan {
                        variable: "e1".to_string(),
                    }),
                }),
                builds: vec![
                    JoinHintNode::Scan {
                        variable: "e2".to_string(),
                    },
                    JoinHintNode::Scan {
                        variable: "e3".to_string(),
                    },
                ],
            },
        };
        let mut enumerator = JoinOrderEnumerator::new();
        let plan = enumerator.plan_with_hint(&qg, &hint).expect("hint plans");
        assert!(matches!(plan, LogicalNodeEnum::WcoIntersect(_)));
    }
}

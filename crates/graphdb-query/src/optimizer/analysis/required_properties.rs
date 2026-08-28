//! Required-property demand analysis (typed property pruning)
//!
//! Propagates property requirements top-down through the plan tree: each
//! consumer declares which `var.prop` columns it needs, and every node
//! records the requirement set its consumers demand of its output. Graph
//! source operators (`GetVertices` / `GetNeighbors` / `ScanVertices` /
//! `ScanEdges` / `GetEdges` / `AppendVertices`) consume the requirement for
//! their binding variable to narrow `projected_properties`.
//!
//! # Typed collection rule
//!
//! Only `Expression::Property { object: Variable(v), property }` references
//! contribute a prunable requirement. Every other occurrence of a variable
//! (bare reference, opaque property object, function argument, alias) marks
//! the variable as *full-value*: its whole column set must be preserved, so
//! the analyzer never narrows a binding that is consumed in a non-prunable
//! way. This keeps the narrowing sound for computed expressions, aliases,
//! and path semantics.

use std::collections::{BTreeSet, HashMap};

use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::PlanNodeEnum;
use graphdb_core::types::expr::visitor::ExpressionVisitor;
use graphdb_core::types::ContextualExpression;
use graphdb_core::Expression;

/// Property requirement for one variable binding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PropertyRequirement {
    /// Variable (alias) the properties belong to.
    pub alias: String,
    /// Resolved tag / edge type when the binding is single-typed.
    pub tag_name: Option<String>,
    /// Property names demanded of the binding.
    pub prop_names: BTreeSet<String>,
    /// The variable is consumed in a non-prunable position (bare reference,
    /// opaque object, function argument, ...): its whole value is needed,
    /// so the binding must not be narrowed. Sticky: once set it is never
    /// cleared by later property references.
    pub full_value: bool,
}

impl PropertyRequirement {
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            ..Default::default()
        }
    }

    /// Merge another requirement (same alias) into this one.
    pub fn merge(&mut self, other: &PropertyRequirement) {
        self.tag_name = other.tag_name.clone().or_else(|| self.tag_name.clone());
        self.prop_names.extend(other.prop_names.iter().cloned());
        self.full_value |= other.full_value;
    }

    /// Whether the binding is narrowable at all.
    pub fn is_narrowable(&self) -> bool {
        !self.full_value && !self.prop_names.is_empty()
    }
}

/// Result of the required-property analysis: one requirement list per plan
/// node id, holding the properties its consumers demand.
#[derive(Debug, Clone, Default)]
pub struct RequiredPropertiesMap {
    requirements: HashMap<i64, Vec<PropertyRequirement>>,
}

impl RequiredPropertiesMap {
    /// Record (replace) the requirement list for a node.
    pub fn record(&mut self, node_id: i64, requirements: &[PropertyRequirement]) {
        self.requirements.insert(node_id, requirements.to_vec());
    }

    /// All requirements recorded for a node.
    pub fn get(&self, node_id: i64) -> Option<&[PropertyRequirement]> {
        self.requirements.get(&node_id).map(Vec::as_slice)
    }

    /// The merged requirement for `var` at `node_id`, if any.
    pub fn requirement_for_var(&self, node_id: i64, var: &str) -> Option<PropertyRequirement> {
        let mut merged: Option<PropertyRequirement> = None;
        for req in self.requirements.get(&node_id)? {
            if req.alias == var {
                match merged.as_mut() {
                    Some(acc) => acc.merge(req),
                    None => merged = Some(req.clone()),
                }
            }
        }
        merged
    }

    /// Properties safe to narrow `node_id`'s source to for `var`.
    ///
    /// Returns `Some(sorted props)` only when the requirement is narrowable:
    /// the variable is not consumed in a full-value position and at least
    /// one property is provably needed. An empty or full-value requirement
    /// yields `None`, so callers keep the full read (empty
    /// `projected_properties` means "read everything").
    pub fn narrowable_properties(&self, node_id: i64, var: &str) -> Option<Vec<String>> {
        let req = self.requirement_for_var(node_id, var)?;
        if !req.is_narrowable() {
            return None;
        }
        Some(req.prop_names.into_iter().collect())
    }
}

/// Per-expression collection result.
///
/// `props` holds prunable `(var, prop)` references; `full_vars` holds
/// variables that appear in a non-prunable position (bare reference, opaque
/// property object, function argument, ...) and therefore need their whole
/// value.
#[derive(Debug, Clone, Default)]
struct CollectedRefs {
    props: Vec<(String, String)>,
    full_vars: BTreeSet<String>,
}

impl CollectedRefs {
    fn visit(expr: &Expression) -> Self {
        let mut collector = RefCollector::default();
        collector.visit(expr);
        collector.into()
    }
}

/// Collects prunable `var.prop` references and full-value variable markers.
///
/// The visitor's default `visit_property` recurses into the object; this
/// override instead treats a `Variable` object as a prunable reference and
/// only recurses for opaque objects (whose inner variables need full
/// values).
#[derive(Debug, Default)]
struct RefCollector {
    props: Vec<(String, String)>,
    full_vars: BTreeSet<String>,
}

impl ExpressionVisitor for RefCollector {
    fn visit_property(&mut self, object: &Expression, property: &str) {
        match object {
            Expression::Variable(var) => {
                self.props.push((var.clone(), property.to_string()));
            }
            other => self.visit(other),
        }
    }

    fn visit_variable(&mut self, name: &str) {
        self.full_vars.insert(name.to_string());
    }
}

impl From<RefCollector> for CollectedRefs {
    fn from(collector: RefCollector) -> Self {
        Self {
            props: collector.props,
            full_vars: collector.full_vars,
        }
    }
}

/// Typed required-property demand analyzer.
#[derive(Debug, Clone, Default)]
pub struct RequiredPropertyAnalyzer;

impl RequiredPropertyAnalyzer {
    pub fn new() -> Self {
        Self
    }

    /// Analyze a plan tree and return the per-node requirement map.
    ///
    /// The traversal is top-down: requirements accumulated from consumers
    /// are merged with each node's own expression references and passed to
    /// the node's children. Graph sources record the requirement for their
    /// binding variable, tagged with the resolved tag / edge type when the
    /// binding is single-typed.
    pub fn analyze(&self, root: &PlanNodeEnum) -> RequiredPropertiesMap {
        let mut map = RequiredPropertiesMap::default();
        self.propagate(root, &[], &mut map);
        map
    }

    /// Propagate requirements down a plan subtree.
    fn propagate(
        &self,
        node: &PlanNodeEnum,
        incoming: &[PropertyRequirement],
        map: &mut RequiredPropertiesMap,
    ) {
        use PlanNodeEnum::*;

        // Leaf graph sources: resolve the binding variable, tag the
        // requirements that match it, and stop.
        match node {
            GetVertices(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    binding_var(n.output_var(), n.src_vids()),
                    single_tag(n.tag_props()),
                );
                return;
            }
            GetNeighbors(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    binding_var(n.output_var(), n.src_vids()),
                    single_tag(n.tag_props()),
                );
                return;
            }
            ScanVertices(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    n.col_names().first().map(String::as_str),
                    n.tag().map(|t| t.to_string()),
                );
                return;
            }
            ScanEdges(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    n.col_names().first().map(String::as_str),
                    n.edge_type().map(|e| e.to_string()),
                );
                return;
            }
            GetEdges(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    Some(n.src()),
                    Some(n.edge_type().to_string()),
                );
                return;
            }
            AppendVertices(n) => {
                self.record_leaf(
                    map,
                    node.id(),
                    incoming,
                    n.input_var(),
                    Some(n.vertex_tag().to_string()),
                );
                return;
            }
            IndexScan(_) | Start(_) | Argument(_) => {
                map.record(node.id(), incoming);
                return;
            }
            _ => {}
        }

        // Intermediate nodes: merge the node's own expression references
        // into the requirements passed down. The node's incoming
        // requirements are recorded as-is (what its consumers demand).
        map.record(node.id(), incoming);
        let mut passed: Vec<PropertyRequirement> = Vec::new();
        for req in incoming {
            passed.push(req.clone());
        }

        match node {
            Project(n) => {
                for col in n.columns() {
                    if let Some(meta) = col.expression.expression() {
                        merge_collected(meta.inner(), &mut passed);
                    }
                }
                // A bare-variable column passes the incoming requirements
                // for that variable through to the input unchanged.
                for col in n.columns() {
                    if let Some(meta) = col.expression.expression() {
                        if let Expression::Variable(var) = meta.inner() {
                            if let Some(req) = incoming.iter().find(|r| r.alias == *var) {
                                merge_requirement(&mut passed, req);
                            }
                        }
                    }
                }
                self.propagate(n.input(), &passed, map);
                for dep in n.dependencies() {
                    self.propagate(dep, &[], map);
                }
            }
            Filter(n) => {
                merge_expr(n.condition(), &mut passed);
                self.propagate(n.input(), &passed, map);
                for dep in n.dependencies() {
                    self.propagate(dep, &[], map);
                }
            }
            Sort(n) => self.propagate_single(n.input(), &passed, map),
            Limit(n) => self.propagate_single(n.input(), &passed, map),
            TopN(n) => self.propagate_single(n.input(), &passed, map),
            Sample(n) => self.propagate_single(n.input(), &passed, map),
            Dedup(n) => self.propagate_single(n.input(), &passed, map),
            Aggregate(n) => self.propagate_single(n.input(), &passed, map),
            Window(n) => self.propagate_single(n.input(), &passed, map),
            Unwind(n) => self.propagate_single(n.input(), &passed, map),
            Materialize(n) => self.propagate_single(n.input(), &passed, map),
            DataCollect(n) => self.propagate_single(n.input(), &passed, map),
            Remove(n) => self.propagate_single(n.input(), &passed, map),

            // Binary join nodes: the join key expressions may reference
            // properties; both inputs receive the full requirement set and
            // each leaf keeps only the entries matching its binding.
            InnerJoin(n) => {
                Self::merge_join_keys(n.hash_keys(), n.probe_keys(), &mut passed);
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            LeftJoin(n) => {
                Self::merge_join_keys(n.hash_keys(), n.probe_keys(), &mut passed);
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            FullOuterJoin(n) => {
                Self::merge_join_keys(n.hash_keys(), n.probe_keys(), &mut passed);
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            RightJoin(n) => {
                Self::merge_join_keys(n.hash_keys(), n.probe_keys(), &mut passed);
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            SemiJoin(n) => {
                Self::merge_join_keys(n.hash_keys(), n.probe_keys(), &mut passed);
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            CrossJoin(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }

            // Set operations: every input provides the same columns.
            Union(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            Minus(n) => {
                self.propagate(n.input(), &passed, map);
                self.propagate(n.minus_input(), &passed, map);
            }
            Intersect(n) => {
                self.propagate(n.input(), &passed, map);
                self.propagate(n.intersect_input(), &passed, map);
            }

            // Traversal nodes pass requirements through to their inputs.
            Expand(n) => {
                for dep in n.inputs() {
                    self.propagate(dep, &passed, map);
                }
            }
            ExpandAll(n) => {
                for dep in n.inputs() {
                    self.propagate(dep, &passed, map);
                }
            }
            Traverse(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            BiExpand(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            BiTraverse(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }

            // Pattern apply: the left input is the main pipeline; the right
            // input is the subquery pattern (correlated columns are part of
            // the subquery's own expressions).
            PatternApply(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &[], map);
            }
            CorrelatedApply(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &[], map);
            }
            RollUpApply(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &[], map);
            }

            // Control flow nodes: propagate into every branch.
            Loop(n) => {
                if let Some(body) = n.body() {
                    self.propagate(body, &passed, map);
                }
            }
            Select(n) => {
                if let Some(branch) = n.if_branch() {
                    self.propagate(branch, &passed, map);
                }
                if let Some(branch) = n.else_branch() {
                    self.propagate(branch, &passed, map);
                }
            }

            // Algorithm nodes: pass requirements into their inputs.
            ShortestPath(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            MultiShortestPath(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            BFSShortest(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }
            AllPaths(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }

            // Data processing nodes.
            Assign(n) => {
                for dep in n.dependencies() {
                    self.propagate(dep, &passed, map);
                }
            }

            // Remaining nodes have no property-consuming expressions and no
            // graph source children; requirements stop here.
            _ => {}
        }
    }

    fn propagate_single(
        &self,
        input: &PlanNodeEnum,
        passed: &[PropertyRequirement],
        map: &mut RequiredPropertiesMap,
    ) {
        self.propagate(input, passed, map);
    }

    /// Merge the property references of join hash/probe keys into the
    /// requirement list so keyed columns are never pruned away.
    fn merge_join_keys(
        hash_keys: &[ContextualExpression],
        probe_keys: &[ContextualExpression],
        passed: &mut Vec<PropertyRequirement>,
    ) {
        for key in hash_keys.iter().chain(probe_keys) {
            merge_expr(key, passed);
        }
    }

    /// Record the requirements demanded of a leaf graph source, tagged with
    /// the binding's resolved tag / edge type.
    fn record_leaf(
        &self,
        map: &mut RequiredPropertiesMap,
        node_id: i64,
        incoming: &[PropertyRequirement],
        binding_var: Option<&str>,
        tag_name: Option<String>,
    ) {
        let Some(var) = binding_var else {
            return;
        };
        let mut resolved: Vec<PropertyRequirement> = Vec::new();
        for req in incoming {
            if req.alias == var {
                let mut tagged = req.clone();
                tagged.tag_name = tag_name.clone();
                resolved.push(tagged);
            }
        }
        if !resolved.is_empty() {
            map.record(node_id, &resolved);
        }
    }
}

/// The binding variable of a graph source, preferring the planner-assigned
/// output variable and falling back to a single non-constant `src_vids`.
///
/// Shared with the projection pushdown rules that narrow graph operator
/// `projected_properties`.
pub fn binding_var<'a>(output_var: Option<&'a str>, src_vids: &'a str) -> Option<&'a str> {
    if let Some(var) = output_var {
        if !var.is_empty() {
            return Some(var);
        }
    }
    let trimmed = src_vids.trim();
    if trimmed.is_empty() || trimmed.split(',').count() != 1 {
        return None;
    }
    // Constant vertex ids are numeric literals, not binding variables.
    if trimmed.parse::<i64>().is_ok() {
        return None;
    }
    Some(trimmed)
}

/// The single tag of a `TagProp` list, or `None` for multi-tag bindings.
fn single_tag(tag_props: &[crate::planning::plan::core::common::TagProp]) -> Option<String> {
    let mut tags = tag_props
        .iter()
        .map(|tp| tp.tag.clone())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    if tags.len() == 1 {
        tags.pop()
    } else {
        None
    }
}

/// Merge the collected references of `expr` into the requirement list,
/// tracking which variables need their full value.
fn merge_collected(expr: &Expression, passed: &mut Vec<PropertyRequirement>) {
    let collected = CollectedRefs::visit(expr);
    for (var, prop) in collected.props {
        merge_property(passed, &var, &prop);
    }
    for var in collected.full_vars {
        // A full-value variable suppresses pruning: the requirement flag is
        // sticky, so later property references cannot re-open the binding.
        merge_property(passed, &var, "");
    }
}

/// Merge the property references of a context expression into the
/// requirement list.
fn merge_expr(expr: &ContextualExpression, passed: &mut Vec<PropertyRequirement>) {
    let Some(meta) = expr.expression() else {
        return;
    };
    merge_collected(meta.inner(), passed);
}

/// Add (or merge into) the requirement for `var` and property `prop`.
///
/// An empty `prop` marks the variable as full-value (non-narrowable).
fn merge_property(passed: &mut Vec<PropertyRequirement>, var: &str, prop: &str) {
    if let Some(req) = passed.iter_mut().find(|r| r.alias == var) {
        if prop.is_empty() {
            req.full_value = true;
        } else {
            req.prop_names.insert(prop.to_string());
        }
    } else {
        let mut req = PropertyRequirement::new(var);
        if prop.is_empty() {
            req.full_value = true;
        } else {
            req.prop_names.insert(prop.to_string());
        }
        passed.push(req);
    }
}

/// Merge a whole requirement into the list.
fn merge_requirement(passed: &mut Vec<PropertyRequirement>, req: &PropertyRequirement) {
    match passed.iter_mut().find(|r| r.alias == req.alias) {
        Some(existing) => existing.merge(req),
        None => passed.push(req.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::{
        GetNeighborsNode, GetVerticesNode, ScanVerticesNode,
    };
    use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::PlanNodeEnum;
    use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
    use graphdb_core::types::expr::ExpressionMeta;
    use graphdb_core::types::operators::BinaryOperator;
    use graphdb_core::{Value, YieldColumn};
    use std::sync::Arc;

    fn contextual(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn prop(var: &str, name: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: name.to_string(),
        }
    }

    fn yield_column(expr: Expression, alias: &str) -> YieldColumn {
        YieldColumn {
            expression: contextual(expr),
            alias: alias.to_string(),
            is_matched: false,
        }
    }

    fn get_vertices(var: &str) -> PlanNodeEnum {
        let mut node = GetVerticesNode::new(1, "test", var);
        node.set_output_var(var.to_string());
        PlanNodeEnum::GetVertices(node)
    }

    #[test]
    fn test_analyzer_collects_property_requirements() {
        // Project(v.age, v.name) <- GetVertices(v)
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![
                    yield_column(prop("v", "age"), "age"),
                    yield_column(prop("v", "name"), "name"),
                ],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        let props = map
            .narrowable_properties(vid, "v")
            .expect("vertex must be narrowable");
        assert_eq!(props, vec!["age".to_string(), "name".to_string()]);
    }

    #[test]
    fn test_analyzer_collects_filter_requirements() {
        // Filter(v.age > 30) <- GetVertices(v)
        let filter = PlanNodeEnum::Filter(
            FilterNode::new(
                get_vertices("v"),
                contextual(Expression::Binary {
                    left: Box::new(prop("v", "age")),
                    op: BinaryOperator::GreaterThan,
                    right: Box::new(Expression::Literal(Value::Int(30))),
                }),
            )
            .expect("filter node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&filter);
        let PlanNodeEnum::Filter(f) = &filter else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(f.input().id(), "v")
            .expect("vertex must be narrowable");
        assert_eq!(props, vec!["age".to_string()]);
    }

    #[test]
    fn test_analyzer_bare_variable_blocks_narrowing() {
        // Project(v AS v) <- GetVertices(v): the bare variable needs the
        // full vertex, so the binding must not be narrowable.
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(Expression::Variable("v".to_string()), "v")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        assert!(map.narrowable_properties(vid, "v").is_none());
    }

    #[test]
    fn test_analyzer_function_argument_blocks_narrowing() {
        // Project(id(v) AS id) <- GetVertices(v): function arguments need
        // the full vertex, so the binding must not be narrowable.
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                get_vertices("v"),
                vec![yield_column(
                    Expression::Function {
                        name: "id".to_string(),
                        args: vec![Expression::Variable("v".to_string())],
                    },
                    "id",
                )],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let vid = match &project {
            PlanNodeEnum::Project(p) => p.input().id(),
            _ => unreachable!(),
        };
        assert!(map.narrowable_properties(vid, "v").is_none());
    }

    #[test]
    fn test_analyzer_matches_binding_variable() {
        // Project(v.age, w.name) <- InnerJoin(GetVertices(v), GetVertices(w)):
        // each binding is narrowed to its own requirements only.
        let left = get_vertices("v");
        let right = get_vertices("w");
        let join = crate::planning::plan::core::nodes::join::InnerJoinNode::new(
            left,
            right,
            vec![],
            vec![],
        )
        .expect("join node");
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::InnerJoin(join),
                vec![
                    yield_column(prop("v", "age"), "age"),
                    yield_column(prop("w", "name"), "name"),
                ],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let PlanNodeEnum::InnerJoin(join) = p.input() else {
            unreachable!()
        };
        let props_v = map
            .narrowable_properties(join.left_input().id(), "v")
            .expect("left binding must be narrowable");
        let props_w = map
            .narrowable_properties(join.right_input().id(), "w")
            .expect("right binding must be narrowable");
        assert_eq!(props_v, vec!["age".to_string()]);
        assert_eq!(props_w, vec!["name".to_string()]);
    }

    #[test]
    fn test_analyzer_tag_resolution() {
        // A single-tag GetVertices resolves the tag on the requirement.
        let mut node = GetVerticesNode::new(1, "test", "v");
        node.set_output_var("v".to_string());
        node.set_tag_props(vec![crate::planning::plan::core::common::TagProp {
            tag: "person".to_string(),
            props: vec!["age".to_string()],
        }]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::GetVertices(node),
                vec![yield_column(prop("v", "age"), "age")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let vid = p.input().id();
        let req = map
            .requirement_for_var(vid, "v")
            .expect("requirement must exist");
        assert_eq!(req.tag_name.as_deref(), Some("person"));
    }

    #[test]
    fn test_analyzer_get_neighbors_narrowing() {
        let mut node = GetNeighborsNode::new(1, "v");
        node.set_output_var("v".to_string());
        let neighbors = PlanNodeEnum::GetNeighbors(node);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                neighbors.clone(),
                vec![yield_column(prop("v", "city"), "city")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(p.input().id(), "v")
            .expect("neighbors must be narrowable");
        assert_eq!(props, vec!["city".to_string()]);
    }

    #[test]
    fn test_scan_vertices_uses_col_names_var() {
        let mut scan = ScanVerticesNode::new(0, "test");
        scan.set_col_names(vec!["n".to_string()]);
        let project = PlanNodeEnum::Project(
            ProjectNode::new(
                PlanNodeEnum::ScanVertices(scan),
                vec![yield_column(prop("n", "age"), "age")],
            )
            .expect("project node"),
        );

        let map = RequiredPropertyAnalyzer::new().analyze(&project);
        let PlanNodeEnum::Project(p) = &project else {
            unreachable!()
        };
        let props = map
            .narrowable_properties(p.input().id(), "n")
            .expect("scan must be narrowable");
        assert_eq!(props, vec!["age".to_string()]);
    }
}

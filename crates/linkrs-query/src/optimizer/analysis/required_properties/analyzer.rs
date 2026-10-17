//! Typed required-property demand analyzer.

use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::PlanNodeEnum;
use linkrs_core::types::ContextualExpression;
use linkrs_core::Expression;

use super::map::RequiredPropertiesMap;
use super::ref_collector::CollectedRefs;
use super::requirement::PropertyRequirement;

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
            }
            Filter(n) => {
                merge_expr(n.condition(), &mut passed);
                self.propagate(n.input(), &passed, map);
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
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            BiTraverse(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
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
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            MultiShortestPath(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            BFSShortest(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
            }
            AllPaths(n) => {
                self.propagate(n.left_input(), &passed, map);
                self.propagate(n.right_input(), &passed, map);
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
            } else if let Some(tag) = tag_name.as_deref() {
                // `TagProperty{tag, prop}` / `EdgeProperty{edge, prop}` references
                // are collected under the tag/edge name; remap them onto the
                // binding variable when the names match so scans can narrow.
                // `full_value` stays sticky: an opaque use still blocks pruning.
                if req.alias == tag {
                    let mut remapped = req.clone();
                    remapped.alias = var.to_string();
                    remapped.tag_name = tag_name.clone();
                    resolved.push(remapped);
                }
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

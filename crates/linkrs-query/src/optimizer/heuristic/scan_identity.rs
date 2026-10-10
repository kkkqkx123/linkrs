//! Scan identity annotation batch.
//!
//! Annotates `ScanVertices` nodes with `identity_only` so the streaming
//! executor can skip boxing a full `Value::Vertex` per row: the entity
//! column carries a lightweight `Value::VertexId` and the per-row property
//! map is never built. Flat property columns are unaffected.
//!
//! The annotation is valid only when the scan's entity variable is never
//! consumed as a whole value downstream: every ancestor reference must be a
//! `var.prop` access served by the scan's flat slots, and no ancestor may
//! structurally require boxed seeds (multi-hop/filtered expands, traversals,
//! whole-entity consumers). Anything unauditable blocks the annotation.
//!
//! The rule is a whole-plan pass mirroring `ExpandPushdownAnnotateRule` and
//! runs in the same batch, after it, so expand `id_only`/`count_only` flags
//! are already final when seed tolerance is judged.
//!
//! The walk mutates the tree in place and never keys decisions by node id:
//! macro-generated plan nodes regenerate ids on clone, so id-keyed matching
//! would silently miss after any clone.

use std::collections::HashSet;

use crate::optimizer::cost::child_accessor::ChildAccessor;
use crate::optimizer::heuristic::context::RewriteContext;
use crate::optimizer::heuristic::expand_pushdown::known_reference_ancestor;
use crate::optimizer::heuristic::pattern::Pattern;
use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::optimizer::heuristic::rule::RewriteRule;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
use linkrs_core::types::expr::visitor::ExpressionVisitor;
use linkrs_core::Expression;

/// Whole-plan rule that annotates `ScanVertices` nodes with `identity_only`.
#[derive(Debug)]
pub struct ScanIdentityAnnotateRule;

impl ScanIdentityAnnotateRule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ScanIdentityAnnotateRule {
    fn default() -> Self {
        Self::new()
    }
}

impl RewriteRule for ScanIdentityAnnotateRule {
    fn name(&self) -> &'static str {
        "ScanIdentityAnnotateRule"
    }

    /// Matches any node; the rule only acts at the plan root.
    fn pattern(&self) -> Pattern {
        Pattern::new()
    }

    fn apply(
        &self,
        ctx: &mut RewriteContext,
        node: &PlanNodeEnum,
    ) -> RewriteResult<Option<TransformResult>> {
        // Whole-plan pass: fire only once at the root.
        if ctx.current_node_id() != 0 {
            return Ok(None);
        }
        let (new_root, changed) = annotate_scan_identity(node);
        if !changed {
            return Ok(None);
        }
        let mut result = TransformResult::new();
        result.erase_curr = true;
        result.add_new_node(new_root);
        Ok(Some(result))
    }
}

/// Annotate every eligible `ScanVertices` in `root` and return the tree.
fn annotate_scan_identity(root: &PlanNodeEnum) -> (PlanNodeEnum, bool) {
    let mut new_root = root.clone();
    let changed = annotate_mut(&mut new_root, &mut Vec::new());
    (new_root, changed)
}

/// Walk the tree top-down, auditing each scan against the ancestor content
/// above it. Ancestors are content snapshots (owned clones): only their
/// expressions, flags and column names are ever read, so clone-regenerated
/// ids are irrelevant.
fn annotate_mut(node: &mut PlanNodeEnum, ancestors: &mut Vec<PlanNodeEnum>) -> bool {
    let mut changed = false;
    if let PlanNodeEnum::ScanVertices(scan) = node {
        let ancestor_refs: Vec<&PlanNodeEnum> = ancestors.iter().collect();
        if !scan.identity_only() && scan_identity_eligible(scan, &ancestor_refs) {
            scan.set_identity_only(true);
            changed = true;
        }
    }
    // Snapshot before descending: the borrow ends before any mutation.
    let snapshot = node.clone();
    ancestors.push(snapshot);
    let child_count = node.children().len();
    for index in 0..child_count {
        if let Some(child) = node.get_child_mut(index) {
            changed |= annotate_mut(child, ancestors);
        }
    }
    ancestors.pop();
    changed
}

/// Whether the scan may emit identity references: the entity variable must
/// have a flat layout (non-empty projection) and every ancestor must pass
/// the audit below.
fn scan_identity_eligible(
    scan: &crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode,
    ancestors: &[&PlanNodeEnum],
) -> bool {
    let Some(var) = scan.col_names().first() else {
        return false;
    };
    // No minimum projection: with an empty projection every property read
    // fails the flat-service check below, so only plans with zero property
    // reads (pure seeds, row counts) can pass the audit — all safe.
    // CTE-tagged scans lower to a working-table source, not storage: the
    // flag would be meaningless there.
    if let Some(tag) = scan.tag() {
        if crate::cte::is_cte_tag(tag) {
            return false;
        }
    }
    let projected: HashSet<&str> = scan
        .projected_properties()
        .iter()
        .map(String::as_str)
        .collect();
    ancestors
        .iter()
        .all(|anc| audit_ancestor(anc, var, &projected))
}

/// Audit one ancestor against the scan variable: every expression reference
/// must be a flat-served property access, and no structural consumer may
/// require boxed seeds.
fn audit_ancestor(anc: &PlanNodeEnum, var: &str, projected: &HashSet<&str>) -> bool {
    if !known_reference_ancestor(anc) {
        return false;
    }
    match anc {
        PlanNodeEnum::Filter(filter) => {
            if !filter.subqueries().is_empty() {
                return false;
            }
            filter
                .condition()
                .get_expression()
                .map(|expr| entity_use_safe(&expr, var, projected))
                .unwrap_or(true)
        }
        PlanNodeEnum::Project(project) => {
            if !project.subqueries().is_empty() {
                return false;
            }
            project.columns().iter().all(|col| {
                col.expression
                    .expression()
                    .map(|meta| entity_use_safe(meta.inner(), var, projected))
                    .unwrap_or(true)
            })
        }
        PlanNodeEnum::Aggregate(agg) => {
            // Grouping by the whole entity needs the full value.
            if agg.group_keys().iter().any(|key| key == var) {
                return false;
            }
            agg.aggregation_args()
                .iter()
                .flatten()
                .all(|expr| entity_use_safe(expr, var, projected))
        }
        PlanNodeEnum::InnerJoin(join) => {
            audit_join_keys(join.hash_keys(), join.probe_keys(), var, projected)
        }
        PlanNodeEnum::LeftJoin(join) => {
            audit_join_keys(join.hash_keys(), join.probe_keys(), var, projected)
        }
        PlanNodeEnum::RightJoin(join) => {
            audit_join_keys(join.hash_keys(), join.probe_keys(), var, projected)
        }
        PlanNodeEnum::FullOuterJoin(join) => {
            audit_join_keys(join.hash_keys(), join.probe_keys(), var, projected)
        }
        PlanNodeEnum::SemiJoin(join) => {
            audit_join_keys(join.hash_keys(), join.probe_keys(), var, projected)
        }
        PlanNodeEnum::ExpandAll(expand) => {
            let filter_safe = expand
                .filter()
                .and_then(|f| f.get_expression())
                .map(|expr| entity_use_safe(&expr, var, projected))
                .unwrap_or(true);
            if !filter_safe {
                return false;
            }
            // The expand consumes the scan variable as its seed only through
            // its first column; a seed-tolerant hop accepts identity rows,
            // anything else needs boxed vertices with tags.
            if expand.col_names().first().map(String::as_str) == Some(var)
                && !seed_tolerant_expand(expand)
            {
                return false;
            }
            true
        }
        // Remaining traversal operators have no seed-tolerant identity path:
        // block when the scan variable flows into their input columns.
        PlanNodeEnum::Expand(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::Traverse(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::BiExpand(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::BiTraverse(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::AppendVertices(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::GetNeighbors(node) => !node.col_names().contains(&var.to_string()),
        PlanNodeEnum::Sort(sort) => sort
            .sort_items()
            .iter()
            .all(|item| entity_use_safe(&item.expression, var, projected)),
        PlanNodeEnum::TopN(topn) => topn
            .sort_items()
            .iter()
            .all(|item| entity_use_safe(&item.expression, var, projected)),
        PlanNodeEnum::Window(window) => window
            .window_functions()
            .iter()
            .flat_map(|wf| {
                wf.args
                    .iter()
                    .chain(wf.partition_by.iter())
                    .chain(wf.order_by.iter())
            })
            .all(|expr| entity_use_safe(expr, var, projected)),
        // Flatten replays child rows without evaluating columns; Limit and
        // Dedup preserve row identity (Dedup hashes rows, and the identity
        // reference hashes like the vertex id it stands for).
        PlanNodeEnum::Flatten(_) | PlanNodeEnum::Limit(_) | PlanNodeEnum::Dedup(_) => true,
        _ => false,
    }
}

/// Whether an expand hop accepts identity rows as seeds: the count-only
/// degree path and the single-step raw-id path both resolve seeds through
/// the tolerant id conversion, while the generic walk needs tagged vertices.
fn seed_tolerant_expand(expand: &ExpandAllNode) -> bool {
    if expand.path_semantic().is_some() {
        return false;
    }
    expand.count_only()
        || (expand.step_limit().unwrap_or(1) == 1
            && expand.filter().is_none()
            && expand.src_vids().is_empty())
}

/// True when `expr` uses `var` only through `var.prop` accesses whose
/// property is served by the scan's flat slots. Any other occurrence (bare
/// reference, function argument, opaque object, subquery body, struct field)
/// needs the whole entity value.
fn entity_use_safe(expr: &Expression, var: &str, projected: &HashSet<&str>) -> bool {
    let mut audit = EntityUseAudit {
        var,
        projected,
        full_value: false,
    };
    audit.visit(expr);
    !audit.full_value
}

struct EntityUseAudit<'a> {
    var: &'a str,
    projected: &'a HashSet<&'a str>,
    full_value: bool,
}

impl ExpressionVisitor for EntityUseAudit<'_> {
    fn visit_variable(&mut self, name: &str) {
        if name == self.var {
            self.full_value = true;
        }
    }

    fn visit_property(&mut self, object: &Expression, property: &str) {
        match object {
            Expression::Variable(name) if name == self.var => {
                if !self.projected.contains(property) {
                    // Served by neither flat slot: the evaluator would fall
                    // back to the boxed entity.
                    self.full_value = true;
                }
            }
            other => self.visit(other),
        }
    }

    fn visit_struct_field(&mut self, base: &Expression, _field: &str) {
        // Struct-field access is not flat-served: any occurrence of the
        // variable underneath needs the whole value.
        self.visit(base);
    }

    fn visit_tag_property(&mut self, tag_name: &str, _property: &str) {
        if tag_name == self.var {
            self.full_value = true;
        }
    }

    fn visit_edge_property(&mut self, edge_name: &str, _property: &str) {
        if edge_name == self.var {
            self.full_value = true;
        }
    }

    fn visit_exists(&mut self, _body: &linkrs_core::types::expr::SubqueryBody) {
        // Subquery bodies are opaque to this audit: a correlated
        // whole-entity use cannot be ruled out.
        self.full_value = true;
    }

    fn visit_in(
        &mut self,
        expr: &Expression,
        _subquery: &linkrs_core::types::expr::SubqueryBody,
        _negated: bool,
    ) {
        self.visit(expr);
        self.full_value = true;
    }

    fn visit_count_subquery(&mut self, _body: &linkrs_core::types::expr::SubqueryBody) {
        self.full_value = true;
    }

    fn visit_scalar_subquery(&mut self, _body: &linkrs_core::types::expr::SubqueryBody) {
        self.full_value = true;
    }
}

fn audit_join_keys(
    hash_keys: &[linkrs_core::types::ContextualExpression],
    probe_keys: &[linkrs_core::types::ContextualExpression],
    var: &str,
    projected: &HashSet<&str>,
) -> bool {
    hash_keys.iter().chain(probe_keys.iter()).all(|key| {
        key.get_expression()
            .map(|expr| entity_use_safe(&expr, var, projected))
            .unwrap_or(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;
    use crate::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode;
    use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
    use crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode;
    use linkrs_core::types::expr::expression_context::ExpressionAnalysisContext;
    use linkrs_core::types::expr::{ContextualExpression, ExpressionMeta};
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::Value;
    use std::sync::Arc;

    fn ctx_expr(expr: Expression) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let id = ctx.register_expression(ExpressionMeta::new(expr));
        ContextualExpression::new(id, ctx)
    }

    fn prop_expr(var: &str, prop: &str) -> Expression {
        Expression::Property {
            object: Box::new(Expression::Variable(var.to_string())),
            property: prop.to_string(),
        }
    }

    fn flat_scan(var: &str, props: &[&str]) -> PlanNodeEnum {
        let mut scan = ScanVerticesNode::new(1, "space");
        scan.set_tag("Node");
        scan.set_col_names(vec![var.to_string()]);
        scan.set_projected_properties(props.iter().map(|s| s.to_string()).collect());
        PlanNodeEnum::ScanVertices(scan)
    }

    fn project_cols(input: PlanNodeEnum, cols: Vec<linkrs_core::YieldColumn>) -> PlanNodeEnum {
        PlanNodeEnum::Project(ProjectNode::new(input, cols).expect("project should build"))
    }

    fn project_prop(input: PlanNodeEnum, var: &str, prop: &str) -> PlanNodeEnum {
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(prop_expr(var, prop)),
            alias: prop.to_string(),
        };
        project_cols(input, vec![col])
    }

    fn project_var(input: PlanNodeEnum, var: &str) -> PlanNodeEnum {
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(Expression::Variable(var.to_string())),
            alias: var.to_string(),
        };
        project_cols(input, vec![col])
    }

    fn hop(vars: [&str; 3], input: PlanNodeEnum) -> PlanNodeEnum {
        let mut expand = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand.set_step_limit(1);
        expand.set_col_names(vars.iter().map(|s| s.to_string()).collect());
        expand.add_input(input);
        PlanNodeEnum::ExpandAll(expand)
    }

    fn scan_flag(root: &PlanNodeEnum) -> bool {
        let mut found = Vec::new();
        fn walk(node: &PlanNodeEnum, out: &mut Vec<bool>) {
            if let PlanNodeEnum::ScanVertices(scan) = node {
                out.push(scan.identity_only());
            }
            for child in node.children() {
                walk(child, out);
            }
        }
        walk(root, &mut found);
        assert_eq!(found.len(), 1, "test plans hold exactly one scan");
        found[0]
    }

    #[test]
    fn flat_projection_enables_identity() {
        // MATCH (a:Node) RETURN a.name
        let plan = project_prop(flat_scan("a", &["name"]), "a", "name");
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(changed, "annotation must change the plan");
        assert!(scan_flag(&annotated));
    }

    #[test]
    fn whole_entity_projection_blocks_identity() {
        // MATCH (a:Node) RETURN a
        let plan = project_var(flat_scan("a", &["name"]), "a");
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(!changed, "annotation must not change the plan");
        assert!(!scan_flag(&annotated));
    }

    #[test]
    fn unprojected_property_use_blocks_identity() {
        // RETURN a.missing served by no flat slot falls back to the box.
        let plan = project_prop(flat_scan("a", &["name"]), "a", "missing");
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(!changed);
        assert!(!scan_flag(&annotated));
    }

    #[test]
    fn entity_only_scan_blocks_identity_on_whole_use() {
        // RETURN a still needs the box even with an entity-only layout.
        let plan = project_var(flat_scan("a", &[]), "a");
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(!changed);
        assert!(!scan_flag(&annotated));
    }

    #[test]
    fn seed_only_scan_without_projection_keeps_identity() {
        // No property reads at all: the scan only feeds a single-step hop
        // seed, so nothing needs the box.
        let plan = hop(["a", "e", "b"], flat_scan("a", &[]));
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(changed);
        assert!(scan_flag(&annotated));
    }

    #[test]
    fn single_step_expand_seed_keeps_identity() {
        // MATCH (a)-[:Link]->(b) RETURN b.name: the scan only feeds a
        // single-step hop seed plus flat uses.
        let plan = project_prop(hop(["a", "e", "b"], flat_scan("a", &["name"])), "b", "name");
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(changed);
        assert!(scan_flag(&annotated));
    }

    #[test]
    fn filtered_expand_seed_blocks_identity() {
        // A filtered hop takes the generic walk, which needs tagged seeds.
        let mut expand = ExpandAllNode::new(1, vec!["Link".to_string()], "OUT");
        expand.set_step_limit(1);
        expand.set_col_names(vec!["a".to_string(), "e".to_string(), "b".to_string()]);
        expand.set_filter(ctx_expr(Expression::Binary {
            left: Box::new(prop_expr("a", "age")),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(3))),
        }));
        expand.add_input(flat_scan("a", &["age"]));
        let plan = PlanNodeEnum::ExpandAll(expand);
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(!changed);
        assert!(!scan_flag(&annotated));
    }

    #[test]
    fn function_argument_blocks_identity() {
        // RETURN labels(a): the function needs the whole entity.
        let col = linkrs_core::YieldColumn {
            expression: ctx_expr(Expression::Function {
                name: "labels".to_string(),
                args: vec![linkrs_core::types::expr::FunctionArg::positional(
                    Expression::Variable("a".to_string()),
                )],
            }),
            alias: "labels".to_string(),
        };
        let plan = project_cols(flat_scan("a", &["name"]), vec![col]);
        let (annotated, changed) = annotate_scan_identity(&plan);
        assert!(!changed);
        assert!(!scan_flag(&annotated));
    }
}

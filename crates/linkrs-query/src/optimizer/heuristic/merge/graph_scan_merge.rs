//! Shared reshaping of graph-scan nodes with a single intermediate operator.

use crate::optimizer::heuristic::result::{RewriteResult, TransformResult};
use crate::planning::plan::core::nodes::access::graph_scan_node::{
    GetNeighborsNode, GetVerticesNode,
};
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;
use linkrs_core::types::ContextualExpression;
use linkrs_core::Expression;

/// Graph-scan nodes whose intermediate `Dedup` or `Project` can be folded into
/// the node itself. Each node keeps its own source-field representation.
pub(crate) trait GraphScanCarrier: Clone {
    fn dependencies(&self) -> &[PlanNodeEnum];
    fn deps_mut(&mut self) -> &mut Vec<PlanNodeEnum>;
    fn dedup(&self) -> bool;
    fn set_dedup(&mut self, enabled: bool);
    fn set_project_source(&mut self, source: ContextualExpression);
    fn wrap(self) -> PlanNodeEnum;
}

impl GraphScanCarrier for GetVerticesNode {
    fn dependencies(&self) -> &[PlanNodeEnum] {
        GetVerticesNode::deps(self)
    }

    fn deps_mut(&mut self) -> &mut Vec<PlanNodeEnum> {
        GetVerticesNode::deps_mut(self)
    }

    fn dedup(&self) -> bool {
        GetVerticesNode::dedup(self)
    }

    fn set_dedup(&mut self, enabled: bool) {
        GetVerticesNode::set_dedup(self, enabled);
    }

    fn set_project_source(&mut self, source: ContextualExpression) {
        self.set_src_ref(source);
    }

    fn wrap(self) -> PlanNodeEnum {
        PlanNodeEnum::GetVertices(self)
    }
}

impl GraphScanCarrier for GetNeighborsNode {
    fn dependencies(&self) -> &[PlanNodeEnum] {
        GetNeighborsNode::deps(self)
    }

    fn deps_mut(&mut self) -> &mut Vec<PlanNodeEnum> {
        GetNeighborsNode::deps_mut(self)
    }

    fn dedup(&self) -> bool {
        GetNeighborsNode::dedup(self)
    }

    fn set_dedup(&mut self, enabled: bool) {
        GetNeighborsNode::set_dedup(self, enabled);
    }

    fn set_project_source(&mut self, source: ContextualExpression) {
        if let Some(expr_meta) = source.expression() {
            if let Expression::Variable(name) = expr_meta.inner() {
                self.set_src_vids(name.clone());
            }
        }
    }

    fn wrap(self) -> PlanNodeEnum {
        PlanNodeEnum::GetNeighbors(self)
    }
}

/// Fold a `Dedup` dependency into the graph-scan node's dedup flag.
pub(crate) fn merge_dedup<N: GraphScanCarrier>(
    node: &PlanNodeEnum,
    extract: impl FnOnce(&PlanNodeEnum) -> Option<&N>,
) -> RewriteResult<Option<TransformResult>> {
    let scan = match extract(node) {
        Some(scan) => scan,
        None => return Ok(None),
    };
    let deps = scan.dependencies();
    if deps.is_empty() {
        return Ok(None);
    }
    let dedup_input = match deps.first() {
        Some(PlanNodeEnum::Dedup(dedup)) => dedup.input().clone(),
        _ => return Ok(None),
    };

    let mut merged = scan.clone();
    if !merged.dedup() {
        merged.set_dedup(true);
    }
    merged.deps_mut().clear();
    merged.deps_mut().push(dedup_input);

    let mut result = TransformResult::new();
    result.erase_curr = true;
    result.add_new_node(merged.wrap());
    Ok(Some(result))
}

/// Fold a single-column `Project` dependency into the graph-scan source.
pub(crate) fn merge_project<N: GraphScanCarrier>(
    node: &PlanNodeEnum,
    extract: impl FnOnce(&PlanNodeEnum) -> Option<&N>,
) -> RewriteResult<Option<TransformResult>> {
    let scan = match extract(node) {
        Some(scan) => scan,
        None => return Ok(None),
    };
    let deps = scan.dependencies();
    if deps.is_empty() {
        return Ok(None);
    }
    let project = match deps.first() {
        Some(PlanNodeEnum::Project(project)) => project,
        _ => return Ok(None),
    };
    let columns = project.columns();
    if columns.len() != 1 {
        return Ok(None);
    }

    let project_input = project.input().clone();
    let mut merged = scan.clone();
    merged.set_project_source(columns[0].expression.clone());
    merged.deps_mut().clear();
    merged.deps_mut().push(project_input);

    let mut result = TransformResult::new();
    result.erase_curr = true;
    result.add_new_node(merged.wrap());
    Ok(Some(result))
}

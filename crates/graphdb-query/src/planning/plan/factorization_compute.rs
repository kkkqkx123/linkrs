mod access;
mod assign;
mod control_flow;
mod flat_leaf;
mod join;
mod operation;
mod set_ops;
mod traversal;
mod unwind;

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::ExpressionId;

use crate::planning::plan::factorization::{
    FGroupPos, FactorizationError, FactorizedSchema, FactorizedSchemaCompute,
};

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

pub(super) fn resolve_id(expr: &ContextualExpression) -> ExpressionId {
    expr.id().clone()
}

/// Register bare output names on an expansion output group.
///
/// Traversal nodes fan out into a new unflat group but carry no tracked
/// `ExpressionId` for the output. Downstream `Variable` references resolve
/// by name through `expression_name_to_group`, so the aliases must be
/// recorded explicitly. Without this the output group stays empty and every
/// downstream use falls back to flatten-all via
/// `GroupDependencyAnalyzer::mark_unresolved`.
pub(super) fn register_output_names(
    schema: &mut FactorizedSchema,
    output_var: Option<&str>,
    col_names: &[String],
    group: FGroupPos,
) -> Result<(), FactorizationError> {
    if let Some(var) = output_var {
        schema.insert_name_for_group(var.to_string(), group)?;
    }
    for name in col_names {
        schema.insert_name_for_group(name.clone(), group)?;
    }
    Ok(())
}

/// Split binary child schemas into `(left, right)`, defaulting missing sides
/// to an empty schema so key-aware join helpers never index out of bounds.
fn pair(child_schemas: &[FactorizedSchema]) -> (&FactorizedSchema, &FactorizedSchema) {
    static EMPTY: std::sync::OnceLock<FactorizedSchema> = std::sync::OnceLock::new();
    let empty = EMPTY.get_or_init(FactorizedSchema::new);
    let left = child_schemas.first().unwrap_or(empty);
    let right = child_schemas.get(1).unwrap_or(empty);
    (left, right)
}

/// Handling module for each logical operator in `compute_factorized_schema`.
///
/// This match is intentionally exhaustive with no wildcard arm: adding a new
/// `LogicalNodeEnum` variant fails compilation here, which forces the author
/// to update both this table and the `FactorizationRewriter::visit_operator`
/// dispatch (R5 drift guard). The returned string names the compute submodule
/// that owns the operator.
pub(crate) fn dispatch_module(node: &LogicalNodeEnum) -> &'static str {
    match node {
        LogicalNodeEnum::ScanVertices(_) => "access",
        LogicalNodeEnum::ScanEdges(_) => "access",
        LogicalNodeEnum::GetVertices(_) => "access",
        LogicalNodeEnum::GetEdges(_) => "access",
        LogicalNodeEnum::GetNeighbors(_) => "access",
        LogicalNodeEnum::Start(_) => "access",
        LogicalNodeEnum::Project(_) => "operation",
        LogicalNodeEnum::Filter(_) => "operation",
        LogicalNodeEnum::Aggregate(_) => "operation",
        LogicalNodeEnum::Flatten(_) => "operation",
        LogicalNodeEnum::Sort(_) => "operation",
        LogicalNodeEnum::TopN(_) => "operation",
        LogicalNodeEnum::Window(_) => "operation",
        LogicalNodeEnum::Dedup(_) => "operation",
        LogicalNodeEnum::Limit(_) => "operation",
        LogicalNodeEnum::Skip(_) => "operation",
        LogicalNodeEnum::Sample(_) => "operation",
        LogicalNodeEnum::InnerJoin(_) => "join",
        LogicalNodeEnum::LeftJoin(_) => "join",
        LogicalNodeEnum::RightJoin(_) => "join",
        LogicalNodeEnum::CrossJoin(_) => "join",
        LogicalNodeEnum::FullOuterJoin(_) => "join",
        LogicalNodeEnum::SemiJoin(_) => "join",
        LogicalNodeEnum::Traverse(_) => "traversal",
        LogicalNodeEnum::Expand(_) => "traversal",
        LogicalNodeEnum::ExpandAll(_) => "traversal",
        LogicalNodeEnum::AppendVertices(_) => "traversal",
        LogicalNodeEnum::BiExpand(_) => "traversal",
        LogicalNodeEnum::BiTraverse(_) => "traversal",
        LogicalNodeEnum::Union(_) => "set_ops",
        LogicalNodeEnum::Minus(_) => "set_ops",
        LogicalNodeEnum::Intersect(_) => "set_ops",
        LogicalNodeEnum::WcoIntersect(_) => "set_ops",
        LogicalNodeEnum::Assign(_) => "assign",
        LogicalNodeEnum::Select(_) => "control_flow",
        LogicalNodeEnum::Loop(_) => "control_flow",
        LogicalNodeEnum::BeginTransaction(_) => "control_flow",
        LogicalNodeEnum::Commit(_) => "control_flow",
        LogicalNodeEnum::Rollback(_) => "control_flow",
        LogicalNodeEnum::PassThrough(_) => "control_flow",
        LogicalNodeEnum::Argument(_) => "control_flow",
        LogicalNodeEnum::Unwind(_) => "unwind",
        LogicalNodeEnum::FulltextSearch(_) => "flat_leaf",
        LogicalNodeEnum::FulltextLookup(_) => "flat_leaf",
        LogicalNodeEnum::MatchFulltext(_) => "flat_leaf",
        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorSearch(_) => "flat_leaf",
        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorLookup(_) => "flat_leaf",
        #[cfg(feature = "vector")]
        LogicalNodeEnum::VectorMatch(_) => "flat_leaf",
        LogicalNodeEnum::InsertVertices(_) => "flat_leaf",
        LogicalNodeEnum::InsertEdges(_) => "flat_leaf",
        LogicalNodeEnum::Update(_) => "flat_leaf",
        LogicalNodeEnum::DeleteVertices(_) => "flat_leaf",
        LogicalNodeEnum::DeleteEdges(_) => "flat_leaf",
        LogicalNodeEnum::DeleteIndex(_) => "flat_leaf",
        LogicalNodeEnum::PipeDeleteVertices(_) => "flat_leaf",
        LogicalNodeEnum::PipeDeleteEdges(_) => "flat_leaf",
        LogicalNodeEnum::CopyFrom(_) => "flat_leaf",
        LogicalNodeEnum::CopyTo(_) => "flat_leaf",
        LogicalNodeEnum::Remove(_) => "flat_leaf",
        LogicalNodeEnum::DataCollect(_) => "flat_leaf",
        LogicalNodeEnum::Materialize(_) => "flat_leaf",
        LogicalNodeEnum::RollUpApply(_) => "flat_leaf",
        LogicalNodeEnum::Apply(_) => "flat_leaf",
        LogicalNodeEnum::PatternApply(_) => "flat_leaf",
        LogicalNodeEnum::CorrelatedApply(_) => "flat_leaf",
        LogicalNodeEnum::MultiShortestPath(_) => "flat_leaf",
        LogicalNodeEnum::BFSShortest(_) => "flat_leaf",
        LogicalNodeEnum::AllPaths(_) => "flat_leaf",
        LogicalNodeEnum::ShortestPath(_) => "flat_leaf",
    }
}

/// Schema for bidirectional expansion over two child schemas.
///
/// Child order follows the node inputs: the first child is the probe side
/// and is flattened before fan-out, the second child is the build side
/// whose bindings are merged into scope. Any remaining nested group is
/// flattened so the expansion output group is the single unflat group.
/// `output_aliases` are registered on the new group via
/// `register_output_names` semantics so downstream variables resolve
/// precisely instead of via flatten-all fallback.
pub(super) fn bi_expand_schema(
    child_schemas: &[FactorizedSchema],
    output_aliases: &[String],
) -> Result<FactorizedSchema, FactorizationError> {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    if schema.has_unflat_group() {
        if let Some(pos) = schema.unflat_group_pos() {
            schema.flatten_group(pos)?;
        }
    }
    if let Some(build) = child_schemas.get(1) {
        let mapping = schema.merge_groups_from(build);
        for (expr_id, gpos) in build.expression_to_group_iter() {
            let new_pos = mapping.get(gpos).copied().unwrap_or(*gpos);
            schema.insert_to_scope_may_repeat(expr_id.clone(), new_pos)?;
        }
        if schema.has_unflat_group() {
            if let Some(pos) = schema.unflat_group_pos() {
                schema.flatten_group(pos)?;
            }
        }
    }
    let out = schema.create_group();
    for alias in output_aliases {
        schema.insert_name_for_group(alias.clone(), out)?;
    }
    schema.validate_at_most_one_unflat()?;
    Ok(schema)
}

impl FactorizedSchemaCompute for LogicalNodeEnum {
    fn compute_factorized_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> Result<FactorizedSchema, FactorizationError> {
        // Keep the R5 drift table linked: every operator must have a dispatch
        // entry, otherwise the tables have drifted.
        debug_assert!(!dispatch_module(self).is_empty());
        let schema = match self {
            LogicalNodeEnum::ScanVertices(n) => access::scan_vertices(n)?,
            LogicalNodeEnum::ScanEdges(n) => access::scan_edges(n)?,
            LogicalNodeEnum::GetVertices(n) => access::get_vertices(n, child_schemas)?,
            LogicalNodeEnum::GetEdges(n) => access::get_edges(n)?,
            LogicalNodeEnum::GetNeighbors(n) => access::get_neighbors(n, child_schemas)?,
            LogicalNodeEnum::Start(_) => access::start()?,

            LogicalNodeEnum::Project(n) => operation::project(n, child_schemas)?,
            LogicalNodeEnum::Filter(n) => operation::filter(n, child_schemas)?,
            LogicalNodeEnum::Aggregate(n) => operation::aggregate(n, child_schemas)?,
            LogicalNodeEnum::Flatten(n) => operation::flatten(n, child_schemas)?,
            LogicalNodeEnum::Sort(n) => operation::sort(n, child_schemas)?,
            LogicalNodeEnum::TopN(n) => operation::top_n(n, child_schemas)?,
            LogicalNodeEnum::Window(n) => operation::window(n, child_schemas)?,
            LogicalNodeEnum::Dedup(n) => operation::dedup(n, child_schemas)?,
            LogicalNodeEnum::Limit(n) => operation::limit(n, child_schemas)?,
            LogicalNodeEnum::Skip(n) => operation::skip(n, child_schemas)?,
            LogicalNodeEnum::Sample(n) => operation::sample(n, child_schemas)?,

            LogicalNodeEnum::InnerJoin(n) => {
                let (left, right) = pair(child_schemas);
                join::binary_join_inner(left, right, &n.hash_keys, &n.probe_keys)?
            }
            LogicalNodeEnum::LeftJoin(n) => {
                let (left, right) = pair(child_schemas);
                join::binary_join_inner(left, right, &n.hash_keys, &n.probe_keys)?
            }
            LogicalNodeEnum::RightJoin(n) => {
                let (left, right) = pair(child_schemas);
                join::binary_join_right(left, right, &n.hash_keys, &n.probe_keys)?
            }
            LogicalNodeEnum::CrossJoin(n) => {
                let (left, right) = pair(child_schemas);
                if n.hash_keys.is_empty() && n.probe_keys.is_empty() {
                    join::cross_join_no_keys(left, right)?
                } else {
                    join::binary_join_inner(left, right, &n.hash_keys, &n.probe_keys)?
                }
            }
            LogicalNodeEnum::FullOuterJoin(n) => {
                let (left, right) = pair(child_schemas);
                join::binary_join_full_outer(left, right, &n.hash_keys, &n.probe_keys)?
            }
            LogicalNodeEnum::SemiJoin(n) => {
                let (left, right) = pair(child_schemas);
                join::binary_join_inner(left, right, &n.hash_keys, &n.probe_keys)?
            }

            LogicalNodeEnum::Traverse(n) => traversal::traverse(n, child_schemas)?,
            LogicalNodeEnum::Expand(n) => traversal::expand(n, child_schemas)?,
            LogicalNodeEnum::ExpandAll(n) => traversal::expand_all(n, child_schemas)?,
            LogicalNodeEnum::AppendVertices(n) => traversal::append_vertices(n, child_schemas)?,
            LogicalNodeEnum::BiExpand(n) => traversal::bi_expand(n, child_schemas)?,
            LogicalNodeEnum::BiTraverse(n) => traversal::bi_traverse(n, child_schemas)?,

            LogicalNodeEnum::Union(_) | LogicalNodeEnum::Minus(_) => {
                set_ops::union_minus(child_schemas)?
            }
            LogicalNodeEnum::Intersect(_) => set_ops::intersect(child_schemas)?,
            LogicalNodeEnum::WcoIntersect(n) => set_ops::wco_intersect(n, child_schemas)?,

            LogicalNodeEnum::Assign(n) => assign::assign(n, child_schemas)?,

            LogicalNodeEnum::Select(n) => control_flow::select(n, child_schemas)?,
            LogicalNodeEnum::Loop(n) => control_flow::loop_node(n, child_schemas)?,
            LogicalNodeEnum::BeginTransaction(_)
            | LogicalNodeEnum::Commit(_)
            | LogicalNodeEnum::Rollback(_)
            | LogicalNodeEnum::PassThrough(_)
            | LogicalNodeEnum::Argument(_) => control_flow::passthrough(child_schemas)?,

            LogicalNodeEnum::Unwind(n) => unwind::unwind(n, child_schemas)?,

            LogicalNodeEnum::FulltextSearch(_)
            | LogicalNodeEnum::FulltextLookup(_)
            | LogicalNodeEnum::MatchFulltext(_) => flat_leaf::flat_leaf()?,
            #[cfg(feature = "vector")]
            LogicalNodeEnum::VectorSearch(_)
            | LogicalNodeEnum::VectorLookup(_)
            | LogicalNodeEnum::VectorMatch(_) => flat_leaf::flat_leaf()?,

            // Single-input barrier: output is fully flat. The rewriter
            // inserts the matching `FlattenAll` nodes explicitly, so compute
            // and rewrite stay in sync (barrier semantics).
            LogicalNodeEnum::Remove(_)
            | LogicalNodeEnum::DataCollect(_)
            | LogicalNodeEnum::Materialize(_)
            | LogicalNodeEnum::RollUpApply(_) => flat_leaf::flatten_all_from_child(child_schemas)?,

            LogicalNodeEnum::InsertVertices(_)
            | LogicalNodeEnum::InsertEdges(_)
            | LogicalNodeEnum::Update(_) => flat_leaf::flat_leaf()?,

            // Binary barrier (Apply family, shortest-path family): no
            // per-operator flatten rule, so merge both children and flatten.
            // The rewriter inserts `FlattenAll` on each side explicitly.
            LogicalNodeEnum::Apply(_)
            | LogicalNodeEnum::PatternApply(_)
            | LogicalNodeEnum::CorrelatedApply(_)
            | LogicalNodeEnum::MultiShortestPath(_)
            | LogicalNodeEnum::BFSShortest(_)
            | LogicalNodeEnum::AllPaths(_)
            | LogicalNodeEnum::ShortestPath(_) => flat_leaf::barrier_binary(child_schemas)?,

            _ => flat_leaf::flatten_all_from_child(child_schemas)?,
        };
        debug_assert!(
            schema.has_at_most_one_unflat(),
            "compute_factorized_schema: at most one unflat group invariant violated"
        );
        Ok(schema)
    }

    fn compute_flat_schema(
        &mut self,
        child_schemas: &[FactorizedSchema],
    ) -> Result<FactorizedSchema, FactorizationError> {
        let flat_children: Vec<FactorizedSchema> = child_schemas
            .iter()
            .map(|cs| cs.flat_copy())
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = self.compute_factorized_schema(&flat_children)?;
        result.flatten_all()?;
        result.validate_at_most_one_unflat()?;
        Ok(result)
    }
}

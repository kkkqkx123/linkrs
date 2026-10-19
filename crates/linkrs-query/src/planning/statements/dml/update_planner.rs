//! Update Operation Planner
//!
//! Query planning for processing UPDATE VERTEX/EDGE statements
//!
//! Migrated to generate a native LogicalNodeEnum tree; `from_logical_root`
//! performs the one-shot logical → physical lowering so the optimizer sees
//! the logical mirror.

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{EdgeUpdateInfo, UpdateTargetType, VertexUpdateInfo};
use crate::planning::plan::logical::logical_nodes::dml::LogicalUpdateNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use std::collections::HashMap;
use std::sync::Arc;

/// Update Operation Planner
/// Responsible for converting UPDATE statements into execution plans.
#[derive(Debug, Clone)]
pub struct UpdatePlanner;

/// Standalone UPDATE has no row scope, so its vid can only be a literal,
/// a query parameter or a session variable. Anything else (bare query
/// variable or compound expression) cannot be evaluated by the single-row
/// source and must fail here with a precise planning error instead of a
/// generic execution-time evaluation failure.
fn validate_bound_standalone_vid(
    vid: &crate::binder::bound::BoundExpression,
) -> Result<(), PlannerError> {
    match vid {
        crate::binder::bound::BoundExpression::Literal(..)
        | crate::binder::bound::BoundExpression::ParameterRef(..)
        | crate::binder::bound::BoundExpression::SessionVariable(..) => Ok(()),
        crate::binder::bound::BoundExpression::Variable(name, _) => {
            Err(PlannerError::PlanGenerationFailed(format!(
                "Standalone UPDATE vid does not support query variable '{name}': use a literal, a parameter (@p) or a session variable ($v)"
            )))
        }
        _ => Err(PlannerError::PlanGenerationFailed(
            "Standalone UPDATE vid must be a literal, a parameter (@p) or a session variable ($v)".to_string(),
        )),
    }
}

impl UpdatePlanner {
    /// Create a new update planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for UpdatePlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let update = match bound {
            BoundStatement::Update(u) => u,
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Statement does not contain UPDATE".to_string(),
                ));
            }
        };

        // Unified entry for expression-level EXISTS / IN (mirrors
        // `transform`): the bound path must also reject subqueries in
        // UPDATE SET values or the UPDATE WHERE condition at planning time
        // with the precise error.
        let space_name = qctx
            .space_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );

        let update_target = match &update.target {
            crate::binder::bound::BoundUpdateTarget::Vertex(_) => {
                return Err(PlannerError::PlanGenerationFailed(
                    "UPDATE vertex requires a tag qualifier (use UPDATE <vid> ON <tag> SET ...)"
                        .to_string(),
                ));
            }
            crate::binder::bound::BoundUpdateTarget::Edge(edge) => {
                let crate::binder::bound::BoundEdgeUpdateTarget {
                    src,
                    dst,
                    edge_type,
                    rank,
                } = &**edge;
                let src_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(src, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;
                let dst_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(dst, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;
                let rank_ctx = rank
                    .as_ref()
                    .map(|r| {
                        crate::binder::expr_converter::bound_expr_to_contextual(r, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let condition = update
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let edge_info = EdgeUpdateInfo {
                    space_name,
                    src: src_ctx,
                    dst: dst_ctx,
                    edge_type: edge_type.clone(),
                    rank: rank_ctx,
                    properties,
                    condition,
                    is_upsert: update.is_upsert,
                    replace_properties: update.assignments.iter().any(|a| a.is_map_overwrite),
                };
                UpdateTargetType::Edge(edge_info)
            }
            crate::binder::bound::BoundUpdateTarget::Tag(tag_name) => {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "UPDATE TAG <{tag_name}> bulk update is not supported yet; use UPDATE <vid> ON <tag> SET ..."
                )));
            }
            crate::binder::bound::BoundUpdateTarget::TagOnVertex { vid, tag_name } => {
                validate_bound_standalone_vid(vid)?;
                let vid_ctx =
                    crate::binder::expr_converter::bound_expr_to_contextual(vid, &expr_ctx)
                        .map_err(PlannerError::PlanGenerationFailed)?;

                let mut properties = HashMap::new();
                for assignment in &update.assignments {
                    let value = crate::binder::expr_converter::bound_expr_to_contextual(
                        &assignment.value,
                        &expr_ctx,
                    )
                    .map_err(PlannerError::PlanGenerationFailed)?;
                    properties.insert(assignment.property.clone(), value);
                }

                let condition = update
                    .where_clause
                    .as_ref()
                    .map(|wc| {
                        crate::binder::expr_converter::bound_expr_to_contextual(wc, &expr_ctx)
                            .map_err(PlannerError::PlanGenerationFailed)
                    })
                    .transpose()?;

                let vertex_info = VertexUpdateInfo {
                    space_name,
                    vertex_id: vid_ctx,
                    tag_name: Some(tag_name.clone()),
                    properties,
                    condition,
                    is_upsert: update.is_upsert,
                    replace_properties: update.assignments.iter().any(|a| a.is_map_overwrite),
                };
                UpdateTargetType::Vertex(vertex_info)
            }
        };

        let logical_root = LogicalNodeEnum::Update(LogicalUpdateNode {
            id: next_node_id(),
            info: update_target,
            output_var: None,
            col_names: vec!["updated".to_string()],
            column_types: vec![],
        });
        Ok(SubPlan::from_logical_root(logical_root))
    }


}

impl Default for UpdatePlanner {
    fn default() -> Self {
        Self::new()
    }
}

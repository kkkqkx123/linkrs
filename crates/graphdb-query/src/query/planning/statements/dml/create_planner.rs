//! CREATE Data Statement Planner
//!
//! Query planning for handling Cypher-style CREATE statements
//! supports CREATE (n:Label {props}) and CREATE (a)-[:Type]->(b) syntaxes

use crate::core::types::ContextualExpression;
use crate::core::Value;
use crate::core::YieldColumn;
use crate::query::parser::ast::{CreateStmt, CreateTarget, Stmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{
        ArgumentNode, EdgeInsertInfo, InsertEdgesNode, InsertVerticesNode, PassThroughNode,
        ProjectNode, TagInsertSpec, VertexInsertInfo,
    },
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::QueryContext;
use std::sync::Arc;

/// CREATE Data Statement Planner
/// Responsible for converting CREATE statements in the Cypher style into execution plans.
#[derive(Debug, Clone)]
pub struct CreatePlanner;

impl CreatePlanner {
    /// Create a new CREATE planner.
    pub fn new() -> Self {
        Self
    }

    /// Determine whether it is a data creation statement (as opposed to a Schema creation statement).
    fn is_data_create(stmt: &CreateStmt) -> bool {
        matches!(
            &stmt.target,
            CreateTarget::Node { .. } | CreateTarget::Edge { .. } | CreateTarget::Path { .. }
        )
    }

    /// Extract the CreateStmt from the Stmt.
    fn extract_create_stmt(&self, stmt: &Stmt) -> Result<CreateStmt, PlannerError> {
        match stmt {
            Stmt::Create(create_stmt) => Ok(create_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement does not contain the CREATE".to_string(),
            )),
        }
    }

    /// Constructing vertex insertion information
    fn build_vertex_insert_info(
        &self,
        space_name: String,
        labels: &[String],
        properties: &[(String, ContextualExpression)],
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<VertexInsertInfo, PlannerError> {
        if labels.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "The CREATE node must specify at least one Label.".to_string(),
            ));
        }

        let tag_specs: Vec<TagInsertSpec> = labels
            .iter()
            .map(|label| TagInsertSpec {
                tag_name: label.clone(),
                prop_names: properties.iter().map(|(k, _)| k.clone()).collect(),
            })
            .collect();

        let prop_values: Vec<ContextualExpression> =
            properties.iter().map(|(_, v)| v.clone()).collect();

        let vid_expr = {
            let expr_meta = crate::core::types::expr::ExpressionMeta::new(
                crate::core::Expression::literal(Value::Null(crate::core::NullType::default())),
            );
            let id = expr_context.register_expression(expr_meta);
            ContextualExpression::new(id, expr_context.clone())
        };

        Ok(VertexInsertInfo {
            space_name,
            tags: tag_specs,
            values: vec![(vid_expr, vec![prop_values])],
            if_not_exists: false,
        })
    }

    /// Constructing edge insertion information
    fn build_edge_insert_info(
        &self,
        space_name: String,
        edge_type: String,
        src_vid: ContextualExpression,
        dst_vid: ContextualExpression,
        properties: &[(String, ContextualExpression)],
    ) -> EdgeInsertInfo {
        let prop_names: Vec<String> = properties.iter().map(|(k, _)| k.clone()).collect();
        let prop_values: Vec<ContextualExpression> =
            properties.iter().map(|(_, v)| v.clone()).collect();

        EdgeInsertInfo {
            space_name,
            edge_name: edge_type,
            prop_names,
            edges: vec![(src_vid, dst_vid, None, prop_values)],
            if_not_exists: false,
        }
    }

    /// Create result projection columns
    fn create_yield_columns(
        &self,
        count: usize,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Vec<YieldColumn> {
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::literal(Value::BigInt(count as i64)),
        );
        let id = expr_context.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, expr_context.clone());

        vec![YieldColumn {
            expression: ctx_expr,
            alias: "created_count".to_string(),
            is_matched: false,
        }]
    }
}

impl Planner for CreatePlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Check the semantic information.
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("CREATE Referenced tags: {:?}", referenced_tags);
        }

        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("CREATE Referenced edge type: {:?}", referenced_edges);
        }

        // Obtain the space name
        let space_name = qctx
            .rctx()
            .space_name
            .clone()
            .unwrap_or_else(|| "default".to_string());

        // Extract the CREATE statement.
        let create_stmt = self.extract_create_stmt(validated.stmt())?;

        // Create a Parameter Node
        let arg_node = ArgumentNode::new(next_node_id(), "create_args");

        // Create the corresponding insertion nodes based on the type of the CREATE target.
        let (insert_node, created_count) = match &create_stmt.target {
            CreateTarget::Node {
                variable: _,
                labels,
                properties,
            } => {
                // Analyzing attributes
                let props = if let Some(expr) = properties {
                    Self::extract_properties(expr, validated.expr_context())?
                } else {
                    vec![]
                };

                let info = self.build_vertex_insert_info(
                    space_name,
                    labels,
                    &props,
                    validated.expr_context(),
                )?;

                (
                    PlanNodeEnum::InsertVertices(InsertVerticesNode::new(next_node_id(), info)),
                    1,
                )
            }
            CreateTarget::Edge {
                variable: _,
                edge_type,
                src,
                dst,
                properties,
                direction: _,
            } => {
                // Analyzing attributes
                let props = if let Some(expr) = properties {
                    Self::extract_properties(expr, validated.expr_context())?
                } else {
                    vec![]
                };

                let info = self.build_edge_insert_info(
                    space_name,
                    edge_type.clone(),
                    src.clone(),
                    dst.clone(),
                    &props,
                );

                (
                    PlanNodeEnum::InsertEdges(InsertEdgesNode::new(next_node_id(), info)),
                    1,
                )
            }
            CreateTarget::Path { patterns } => {
                let mut vertex_infos = Vec::new();
                let mut edge_infos = Vec::new();
                let mut created_count = 0;

                for pattern in patterns {
                    match pattern {
                        crate::query::parser::ast::pattern::Pattern::Path(path) => {
                            let (mut vertices, mut edges) = self.process_path_pattern(
                                path,
                                &space_name,
                                validated.expr_context(),
                            )?;
                            vertex_infos.append(&mut vertices);
                            edge_infos.append(&mut edges);
                            created_count += 1;
                        }
                        crate::query::parser::ast::pattern::Pattern::Node(node) => {
                            let info = self.process_node_pattern(
                                node,
                                &space_name,
                                validated.expr_context(),
                            )?;
                            vertex_infos.push(info);
                            created_count += 1;
                        }
                        _ => {
                            return Err(PlannerError::PlanGenerationFailed(
                                "Path creation only supports node and path modes".to_string(),
                            ));
                        }
                    }
                }

                if vertex_infos.is_empty() && edge_infos.is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "Path creation must contain at least one node or edge".to_string(),
                    ));
                }

                let mut insert_nodes = Vec::new();

                for info in vertex_infos {
                    insert_nodes.push(PlanNodeEnum::InsertVertices(InsertVerticesNode::new(
                        next_node_id(),
                        info,
                    )));
                }

                for info in edge_infos {
                    insert_nodes.push(PlanNodeEnum::InsertEdges(InsertEdgesNode::new(
                        next_node_id(),
                        info,
                    )));
                }

                if insert_nodes.len() == 1 {
                    (
                        insert_nodes
                            .into_iter()
                            .next()
                            .expect("insert_nodes should not be null after length checking"),
                        created_count,
                    )
                } else {
                    let combined = self.combine_insert_nodes(insert_nodes)?;
                    (PlanNodeEnum::PassThrough(combined), created_count)
                }
            }
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "Unsupported CREATE target types".to_string(),
                ));
            }
        };

        // Create a projection node to return the creation results.
        let yield_columns = self.create_yield_columns(created_count, validated.expr_context());

        let project_node = ProjectNode::new(insert_node, yield_columns).map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
        })?;

        let final_node = PlanNodeEnum::Project(project_node);

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(final_node), Some(PlanNodeEnum::Argument(arg_node)));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Create(create_stmt) => Self::is_data_create(create_stmt),
            _ => false,
        }
    }
}

impl CreatePlanner {
    /// Extract attribute key-value pairs from the expression.
    fn extract_properties(
        expr: &ContextualExpression,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<Vec<(String, ContextualExpression)>, PlannerError> {
        if let Some(expr_meta) = expr.expression() {
            if let crate::core::Expression::Map(map) = expr_meta.inner() {
                let mut result = Vec::new();
                for (key, value_expr) in map {
                    let value_meta =
                        crate::core::types::expr::ExpressionMeta::new(value_expr.clone());
                    let id = expr_context.register_expression(value_meta);
                    let ctx_expr = ContextualExpression::new(id, expr_context.clone());
                    result.push((key.clone(), ctx_expr));
                }
                Ok(result)
            } else {
                Err(PlannerError::PlanGenerationFailed(
                    "attribute must be a Map expression".to_string(),
                ))
            }
        } else {
            Err(PlannerError::PlanGenerationFailed(
                "Invalid expression".to_string(),
            ))
        }
    }

    /// Processing node mode
    fn process_node_pattern(
        &self,
        node: &crate::query::parser::ast::pattern::NodePattern,
        space_name: &str,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<VertexInsertInfo, PlannerError> {
        let props = if let Some(ref expr) = node.properties {
            Self::extract_properties(expr, expr_context)?
        } else {
            vec![]
        };

        self.build_vertex_insert_info(space_name.to_string(), &node.labels, &props, expr_context)
    }

    /// Handle path patterns
    fn process_path_pattern(
        &self,
        path: &crate::query::parser::ast::pattern::PathPattern,
        space_name: &str,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Result<(Vec<VertexInsertInfo>, Vec<EdgeInsertInfo>), PlannerError> {
        let mut vertex_infos = Vec::new();
        let mut edge_infos = Vec::new();
        let mut prev_vertex: Option<VertexInsertInfo> = None;

        for element in &path.elements {
            match element {
                crate::query::parser::ast::pattern::PathElement::Node(node) => {
                    let vertex_info = self.process_node_pattern(node, space_name, expr_context)?;
                    prev_vertex = Some(vertex_info.clone());
                    vertex_infos.push(vertex_info);
                }
                crate::query::parser::ast::pattern::PathElement::Edge(edge) => {
                    if prev_vertex.is_none() {
                        return Err(PlannerError::PlanGenerationFailed(
                            "Edge patterns must be preceded by node patterns".to_string(),
                        ));
                    }

                    let props = if let Some(ref expr) = edge.properties {
                        Self::extract_properties(expr, expr_context)?
                    } else {
                        vec![]
                    };

                    if edge.edge_types.is_empty() {
                        return Err(PlannerError::PlanGenerationFailed(
                            "The edge mode must specify the edge type".to_string(),
                        ));
                    }

                    let edge_type = edge.edge_types[0].clone();

                    let src_vid = {
                        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
                            crate::core::Expression::literal(Value::Null(
                                crate::core::NullType::default(),
                            )),
                        );
                        let id = expr_context.register_expression(expr_meta);
                        ContextualExpression::new(id, expr_context.clone())
                    };
                    let dst_vid = {
                        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
                            crate::core::Expression::literal(Value::Null(
                                crate::core::NullType::default(),
                            )),
                        );
                        let id = expr_context.register_expression(expr_meta);
                        ContextualExpression::new(id, expr_context.clone())
                    };

                    let edge_info = EdgeInsertInfo {
                        space_name: space_name.to_string(),
                        edge_name: edge_type,
                        prop_names: props.iter().map(|(k, _)| k.clone()).collect(),
                        edges: vec![(
                            src_vid,
                            dst_vid,
                            None,
                            props.iter().map(|(_, v)| v.clone()).collect(),
                        )],
                        if_not_exists: false,
                    };

                    edge_infos.push(edge_info);
                }
                _ => {
                    return Err(PlannerError::PlanGenerationFailed(
                        "Path creation does not support Alternative, Optional, or Repeated modes."
                            .to_string(),
                    ));
                }
            }
        }

        Ok((vertex_infos, edge_infos))
    }

    /// Combining multiple insertion nodes
    fn combine_insert_nodes(
        &self,
        nodes: Vec<PlanNodeEnum>,
    ) -> Result<PassThroughNode, PlannerError> {
        if nodes.is_empty() {
            return Err(PlannerError::PlanGenerationFailed(
                "Unable to combine empty node lists".to_string(),
            ));
        }

        Ok(PassThroughNode::new(next_node_id()))
    }
}

impl Default for CreatePlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::query::parser::parsing::Parser;
    use crate::query::planning::planner::{Planner, ValidatedStatement};
    use crate::query::validator::ValidationInfo;
    use crate::query::QueryContext;
    use std::sync::Arc;

    #[test]
    fn test_create_path_simple() {
        let sql = "CREATE (a:Person {name: 'Alice'})-[:FRIEND]->(b:Person {name: 'Bob'})";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("parsing failure");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_ok(),
            "CREATE PATH should succeed, but gets the error: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_create_path_with_properties() {
        let sql = "CREATE (a:Person {name: 'Alice', age: 30})-[:FRIEND {since: 2020}]->(b:Person {name: 'Bob', age: 25})";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("parsing failure");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_ok(),
            "The CREATE PATH command with attributes should succeed."
        );
    }

    #[test]
    fn test_create_path_multiple_edges() {
        let sql = "CREATE (a:Person)-[:FRIEND]->(b:Person)-[:FRIEND]->(c:Person)";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("parsing failure");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_ok(),
            "The多边al CREATE PATH operation should succeed."
        );
    }

    #[test]
    fn test_create_path_single_node() {
        let sql = "CREATE (a:Person {name: 'Alice'})";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("parsing failure");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_ok(),
            "The single-node CREATE operation should succeed."
        );
    }

    #[test]
    fn test_create_path_without_labels() {
        let sql = "CREATE (a)-[:FRIEND]->(b)";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("parsing failure");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_err(),
            "A CREATE PATH command without any tags should fail."
        );
    }

    #[test]
    fn test_create_path_bidirectional_edge() {
        let sql = "CREATE (a:Person)-[:FRIEND]-(b:Person)";
        let mut parser = Parser::new(sql);
        let parser_result = parser.parse().expect("Parsing should work.");

        let mut planner = CreatePlanner::new();
        let qctx = Arc::new(QueryContext::default());

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(parser_result.ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(
            result.is_ok(),
            "The creation of a bidirectional edge using the “CREATE PATH” command should succeed."
        );
    }
}

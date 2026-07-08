//! Insert Operation Planner
//!
//! Query planning for INSERT VERTEX and INSERT EDGE statements

use crate::core::types::expr::contextual::ContextualExpression;
use crate::query::parser::ast::{InsertStmt, InsertTarget, Stmt, VertexRow};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{
        ArgumentNode, EdgeInsertInfo, InsertEdgesNode, InsertVerticesNode, TagInsertSpec,
        VertexInsertInfo,
    },
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
use std::sync::Arc;

#[cfg(test)]
use crate::core::YieldColumn;
#[cfg(test)]
use crate::query::parser::ast::utils::ExprFactory;
#[cfg(test)]
use crate::query::validator::context::ExpressionAnalysisContext;

/// Insert Operation Planner
/// Responsible for converting INSERT statements into execution plans.
#[derive(Debug, Clone)]
pub struct InsertPlanner;

impl InsertPlanner {
    /// Create a new insertion planner.
    pub fn new() -> Self {
        Self
    }

    /// Check whether the statements match the insertion operations.
    pub fn match_stmt(stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Insert(_))
    }

    /// Extract the InsertStmt from the Stmt.
    fn extract_insert_stmt(&self, stmt: &Stmt) -> Result<InsertStmt, PlannerError> {
        match stmt {
            Stmt::Insert(insert_stmt) => Ok(insert_stmt.clone()),
            _ => Err(PlannerError::PlanGenerationFailed(
                "statement is not an INSERT statement".to_string(),
            )),
        }
    }

    /// Constructing vertex insertion information
    /// Supports the insertion of multiple tags.
    fn build_vertex_insert_info(
        &self,
        space_name: String,
        tags: Vec<crate::query::parser::ast::TagInsertSpec>,
        values: Vec<VertexRow>,
        if_not_exists: bool,
    ) -> Result<VertexInsertInfo, PlannerError> {
        // Please provide the text you would like to have translated, as well as the specific instructions regarding the conversion of tag specifications. I will then assist you with the translation.
        let tag_specs: Vec<TagInsertSpec> = tags
            .into_iter()
            .map(|tag| TagInsertSpec {
                tag_name: tag.tag_name,
                prop_names: tag.prop_names,
            })
            .collect();

        // Convert `VertexRow` to the format `(vid, Vec<Vec(Expression>>)`
        // Each tag corresponds to a list of attribute values.
        let converted_values: Vec<(ContextualExpression, Vec<Vec<ContextualExpression>>)> = values
            .into_iter()
            .map(|row| (row.vid, row.tag_values))
            .collect();

        Ok(VertexInsertInfo {
            space_name,
            tags: tag_specs,
            values: converted_values,
            if_not_exists,
        })
    }

    /// Constructing edge insertion information
    fn build_edge_insert_info(
        &self,
        space_name: String,
        edge_name: String,
        prop_names: Vec<String>,
        edges: Vec<(
            ContextualExpression,
            ContextualExpression,
            Option<ContextualExpression>,
            Vec<ContextualExpression>,
        )>,
        if_not_exists: bool,
    ) -> EdgeInsertInfo {
        EdgeInsertInfo {
            space_name,
            edge_name,
            prop_names,
            edges,
            if_not_exists,
        }
    }

    /// Create columns for projecting the insertion results.
    #[cfg(test)]
    fn create_yield_columns(
        &self,
        count: usize,
        expr_context: &Arc<ExpressionAnalysisContext>,
    ) -> Vec<YieldColumn> {
        let expr = ExprFactory::constant(
            crate::core::Value::BigInt(count as i64),
            expr_context.clone(),
        );
        vec![YieldColumn::new(expr, "inserted_count".to_string())]
    }
}

impl Planner for InsertPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Obtain the space name
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        // Use the verification information to optimize the planning process.
        let validation_info = &validated.validation_info;

        // Check the semantic information.
        let referenced_tags = &validation_info.semantic_info.referenced_tags;
        if !referenced_tags.is_empty() {
            log::debug!("INSERT references the tag: {:?}", referenced_tags);
        }

        let referenced_edges = &validation_info.semantic_info.referenced_edges;
        if !referenced_edges.is_empty() {
            log::debug!("INSERT referenced edge type: {:?}", referenced_edges);
        }

        // Extract the INSERT statement
        let insert_stmt = self.extract_insert_stmt(validated.stmt())?;

        // Create a Parameter Node
        let arg_node = ArgumentNode::new(next_node_id(), "insert_args");

        // Create the corresponding insertion nodes based on the type of the INSERT target.
        let (insert_node, _inserted_count) = match &insert_stmt.target {
            InsertTarget::Vertices { tags, values } => {
                let count = values.len();
                // Supports the insertion of multiple tags.
                if tags.is_empty() {
                    return Err(PlannerError::PlanGenerationFailed(
                        "INSERT VERTEX must specify at least one tag".to_string(),
                    ));
                }
                let info = self.build_vertex_insert_info(
                    space_name,
                    tags.clone(),
                    values.clone(),
                    insert_stmt.if_not_exists,
                )?;
                (
                    PlanNodeEnum::InsertVertices(InsertVerticesNode::new(next_node_id(), info)),
                    count,
                )
            }
            InsertTarget::Edge {
                edge_name,
                prop_names,
                edges,
            } => {
                let count = edges.len();
                let info = self.build_edge_insert_info(
                    space_name,
                    edge_name.clone(),
                    prop_names.clone(),
                    edges.clone(),
                    insert_stmt.if_not_exists,
                );
                (
                    PlanNodeEnum::InsertEdges(InsertEdgesNode::new(next_node_id(), info)),
                    count,
                )
            }
        };

        // Create a SubPlan with InsertVertices/InsertEdges as the root node
        // Note: ProjectNode is not needed here as the executor returns Count result directly
        let sub_plan = SubPlan::new(Some(insert_node), Some(PlanNodeEnum::Argument(arg_node)));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        Self::match_stmt(stmt)
    }
}

impl Default for InsertPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use crate::core::types::expr::contextual::ContextualExpression;
    use crate::core::Value;
    use crate::query::parser::ast::utils::ExprFactory;
    use crate::query::parser::ast::{Ast, Span, Stmt};
    use crate::query::parser::ast::{InsertStmt, InsertTarget, TagInsertSpec, VertexRow};
    use crate::query::planning::planner::{Planner, ValidatedStatement};
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::query::validator::ValidationInfo;
    use crate::query::QueryContext;
    use std::sync::Arc;

    fn create_test_span() -> Span {
        use crate::core::types::span::Position;
        Span::new(Position::new(1, 1), Position::new(1, 1))
    }

    fn create_test_stmt_with_insert(target: InsertTarget) -> Arc<Ast> {
        let insert_stmt = InsertStmt {
            span: create_test_span(),
            target,
            if_not_exists: false,
        };
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        Arc::new(Ast::new(Stmt::Insert(insert_stmt), ctx))
    }

    fn create_test_qctx() -> Arc<QueryContext> {
        Arc::new(QueryContext::default())
    }

    // Auxiliary function: Creating constant expressions
    fn lit(val: Value) -> ContextualExpression {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        ExprFactory::constant(val, ctx)
    }

    #[test]
    fn test_insert_planner_new() {
        let planner = InsertPlanner::new();
        let ast = create_test_stmt_with_insert(InsertTarget::Vertices {
            tags: vec![TagInsertSpec {
                tag_name: "person".to_string(),
                prop_names: vec!["name".to_string(), "age".to_string()],
                is_default_props: false,
            }],
            values: vec![VertexRow {
                vid: lit(Value::Int(1)),
                tag_values: vec![vec![
                    lit(Value::String("Alice".to_string())),
                    lit(Value::Int(30)),
                ]],
            }],
        });
        assert!(planner.match_planner(&ast.stmt));
    }

    #[test]
    fn test_match_stmt_with_insert() {
        let ast = create_test_stmt_with_insert(InsertTarget::Vertices {
            tags: vec![TagInsertSpec {
                tag_name: "person".to_string(),
                prop_names: vec![],
                is_default_props: true,
            }],
            values: vec![],
        });
        assert!(InsertPlanner::match_stmt(&ast.stmt));
    }

    #[test]
    fn test_match_stmt_without_insert() {
        let stmt = Stmt::Use(crate::query::parser::ast::UseStmt {
            span: create_test_span(),
            space: "test_space".to_string(),
        });
        assert!(!InsertPlanner::match_stmt(&stmt));
    }

    #[test]
    fn test_extract_insert_stmt_success() {
        let planner = InsertPlanner::new();
        let target = InsertTarget::Vertices {
            tags: vec![TagInsertSpec {
                tag_name: "person".to_string(),
                prop_names: vec!["name".to_string()],
                is_default_props: false,
            }],
            values: vec![],
        };
        let stmt = create_test_stmt_with_insert(target.clone());
        let result = planner
            .extract_insert_stmt(&stmt.as_ref().stmt)
            .expect("Failed to extract insert statement");
        assert_eq!(result.target, target);
    }

    #[test]
    fn test_extract_insert_stmt_failure() {
        let planner = InsertPlanner::new();
        let stmt = Stmt::Use(crate::query::parser::ast::UseStmt {
            span: create_test_span(),
            space: "test_space".to_string(),
        });
        let result = planner.extract_insert_stmt(&stmt);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not an INSERT statement"));
    }

    #[test]
    fn test_build_vertex_insert_info() {
        let planner = InsertPlanner::new();
        let info = planner
            .build_vertex_insert_info(
                "test_space".to_string(),
                vec![TagInsertSpec {
                    tag_name: "person".to_string(),
                    prop_names: vec!["name".to_string(), "age".to_string()],
                    is_default_props: false,
                }],
                vec![VertexRow {
                    vid: lit(Value::Int(1)),
                    tag_values: vec![vec![
                        lit(Value::String("Alice".to_string())),
                        lit(Value::Int(30)),
                    ]],
                }],
                false,
            )
            .expect("Failed to build vertex insert info");
        assert_eq!(info.space_name, "test_space");
        assert_eq!(info.tags.len(), 1);
        assert_eq!(info.tags[0].tag_name, "person");
        assert_eq!(info.tags[0].prop_names.len(), 2);
        assert_eq!(info.values.len(), 1);
    }

    #[test]
    fn test_build_edge_insert_info() {
        let planner = InsertPlanner::new();
        let info = planner.build_edge_insert_info(
            "test_space".to_string(),
            "follow".to_string(),
            vec!["since".to_string()],
            vec![(
                lit(Value::Int(1)),
                lit(Value::Int(2)),
                Some(lit(Value::Int(0))),
                vec![lit(Value::String("2023".to_string()))],
            )],
            false,
        );
        assert_eq!(info.space_name, "test_space");
        assert_eq!(info.edge_name, "follow");
        assert_eq!(info.prop_names.len(), 1);
        assert_eq!(info.edges.len(), 1);
    }

    #[test]
    fn test_create_yield_columns() {
        let planner = InsertPlanner::new();
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let columns = planner.create_yield_columns(5, &expr_ctx);
        assert_eq!(columns.len(), 1);
        assert_eq!(columns[0].alias, "inserted_count");
    }

    #[test]
    fn test_transform_insert_vertices() {
        let mut planner = InsertPlanner::new();
        let target = InsertTarget::Vertices {
            tags: vec![TagInsertSpec {
                tag_name: "person".to_string(),
                prop_names: vec!["name".to_string()],
                is_default_props: false,
            }],
            values: vec![
                VertexRow {
                    vid: lit(Value::Int(1)),
                    tag_values: vec![vec![lit(Value::String("Alice".to_string()))]],
                },
                VertexRow {
                    vid: lit(Value::Int(2)),
                    tag_values: vec![vec![lit(Value::String("Bob".to_string()))]],
                },
            ],
        };
        let ast = create_test_stmt_with_insert(target);
        let qctx = create_test_qctx();

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(result.is_ok());
        let sub_plan = result.expect("Failed to transform insert statement");
        assert!(sub_plan.root.is_some());
    }

    #[test]
    fn test_transform_insert_edge() {
        let mut planner = InsertPlanner::new();
        let target = InsertTarget::Edge {
            edge_name: "follow".to_string(),
            prop_names: vec!["since".to_string()],
            edges: vec![(
                lit(Value::Int(1)),
                lit(Value::Int(2)),
                Some(lit(Value::Int(0))),
                vec![lit(Value::String("2023".to_string()))],
            )],
        };
        let ast = create_test_stmt_with_insert(target);
        let qctx = create_test_qctx();

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(result.is_ok());
        let sub_plan = result.expect("Failed to transform insert statement");
        assert!(sub_plan.root.is_some());
    }

    #[test]
    fn test_transform_without_insert_stmt() {
        let mut planner = InsertPlanner::new();
        let stmt = Stmt::Use(crate::query::parser::ast::UseStmt {
            span: create_test_span(),
            space: "test_space".to_string(),
        });
        let qctx = create_test_qctx();
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let ast = Arc::new(Ast::new(stmt, ctx));

        // Create a verified statement.
        let validation_info = ValidationInfo::new();
        let validated = ValidatedStatement::new(ast, validation_info);

        let result = planner.transform(&validated, qctx);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_impl() {
        let planner: InsertPlanner = Default::default();
        let ast = create_test_stmt_with_insert(InsertTarget::Vertices {
            tags: vec![TagInsertSpec {
                tag_name: "test".to_string(),
                prop_names: vec![],
                is_default_props: true,
            }],
            values: vec![],
        });
        assert!(planner.match_planner(&ast.stmt));
    }
}

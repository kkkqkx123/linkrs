use graphdb_core::DBResult;

use crate::binder::bound::*;
use crate::parser::ast::pattern::Pattern;

use super::Binder;

impl Binder {
    pub(crate) fn bind_create(
        &mut self,
        stmt: &crate::parser::ast::CreateStmt,
    ) -> DBResult<BoundStatement> {
        match &stmt.target {
            crate::parser::ast::CreateTarget::Node { .. }
            | crate::parser::ast::CreateTarget::Edge { .. }
            | crate::parser::ast::CreateTarget::Path { .. } => {
                let bound_target = self.bind_create_target(&stmt.target)?;
                Ok(BoundStatement::Create(BoundCreate {
                    target: bound_target,
                    if_not_exists: stmt.if_not_exists,
                }))
            }
            _ => {
                // Schema DDL targets (Tag, EdgeType, Space, Index, Sequence)
                // remain as Other for MaintainPlanner.
                Ok(BoundStatement::Other(Box::new(
                    crate::parser::ast::Stmt::Create(stmt.clone()),
                )))
            }
        }
    }

    fn bind_create_target(
        &mut self,
        target: &crate::parser::ast::CreateTarget,
    ) -> DBResult<BoundCreateTarget> {
        match target {
            crate::parser::ast::CreateTarget::Node {
                variable,
                labels,
                properties,
            } => {
                let bound_props = properties
                    .as_ref()
                    .map(|p| Self::extract_map_properties(p))
                    .transpose()?;
                Ok(BoundCreateTarget::Node {
                    variable: variable.clone(),
                    labels: labels.clone(),
                    properties: bound_props,
                })
            }
            crate::parser::ast::CreateTarget::Edge {
                variable,
                edge_type,
                src,
                dst,
                properties,
                direction,
            } => {
                let src_expr = src
                    .expression()
                    .map(|e| e.inner().clone())
                    .unwrap_or_else(|| graphdb_core::types::Expression::Variable("_".to_string()));
                let dst_expr = dst
                    .expression()
                    .map(|e| e.inner().clone())
                    .unwrap_or_else(|| graphdb_core::types::Expression::Variable("_".to_string()));
                let bound_src = Self::convert_ast_expr_to_bound(&src_expr)?;
                let bound_dst = Self::convert_ast_expr_to_bound(&dst_expr)?;
                let bound_props = properties
                    .as_ref()
                    .map(|p| Self::extract_map_properties(p))
                    .transpose()?;
                Ok(BoundCreateTarget::Edge {
                    variable: variable.clone(),
                    edge_type: edge_type.clone(),
                    src: bound_src,
                    dst: bound_dst,
                    properties: bound_props,
                    direction: *direction,
                })
            }
            crate::parser::ast::CreateTarget::Path { patterns } => {
                let bound_patterns = patterns
                    .iter()
                    .map(|p| self.bind_pattern_element(p))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundCreateTarget::Path {
                    patterns: bound_patterns,
                })
            }
            _ => Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "Unexpected CREATE target type".to_string(),
                ),
            )),
        }
    }

    fn bind_pattern_element(
        &mut self,
        pattern: &Pattern,
    ) -> DBResult<BoundPatternElement> {
        match pattern {
            Pattern::Node(np) => Ok(BoundPatternElement::Node(BoundPatternVertex {
                variable: np.variable.clone(),
                labels: np.labels.clone(),
                properties: np
                    .properties
                    .as_ref()
                    .map(|p| Self::extract_map_properties(p))
                    .transpose()?,
            })),
            Pattern::Edge(ep) => {
                let properties = ep
                    .properties
                    .as_ref()
                    .map(|p| Self::extract_map_properties(p))
                    .transpose()?;
                Ok(BoundPatternElement::Edge(BoundPatternEdge {
                    variable: ep.variable.clone(),
                    edge_types: ep.edge_types.clone(),
                    properties,
                    direction: ep.direction,
                }))
            }
            _ => Err(graphdb_core::error::DBError::from(
                graphdb_core::error::QueryError::invalid_query(
                    "CREATE PATH only supports node or edge patterns".to_string(),
                ),
            )),
        }
    }

    pub(crate) fn bind_drop(
        &mut self,
        stmt: &crate::parser::ast::DropStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Drop(BoundDrop {
            target: stmt.target.clone(),
            if_exists: stmt.if_exists,
        }))
    }

    pub(crate) fn bind_alter(
        &mut self,
        stmt: &crate::parser::ast::AlterStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Alter(BoundAlter {
            target: stmt.target.clone(),
        }))
    }

    pub(crate) fn bind_desc(
        &mut self,
        stmt: &crate::parser::ast::DescStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::Desc(stmt.clone()),
        )))
    }

    pub(crate) fn bind_show_create(
        &mut self,
        stmt: &crate::parser::ast::ShowCreateStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ShowCreate(stmt.clone()),
        )))
    }

    pub(crate) fn bind_clear_space(
        &mut self,
        stmt: &crate::parser::ast::ClearSpaceStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ClearSpace(stmt.clone()),
        )))
    }

    pub(crate) fn bind_begin_transaction(
        &mut self,
        stmt: &crate::parser::ast::BeginTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::BeginTransaction(BoundBeginTransaction {
            read_only: stmt.read_only,
        }))
    }

    pub(crate) fn bind_commit(
        &mut self,
        _stmt: &crate::parser::ast::CommitTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Commit(BoundCommit))
    }

    pub(crate) fn bind_rollback(
        &mut self,
        stmt: &crate::parser::ast::RollbackTransactionStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Rollback(BoundRollback {
            savepoint_name: stmt.savepoint_name.clone(),
        }))
    }

    pub(crate) fn bind_savepoint(
        &mut self,
        stmt: &crate::parser::ast::SavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::Savepoint(stmt.clone()),
        )))
    }

    pub(crate) fn bind_release_savepoint(
        &mut self,
        stmt: &crate::parser::ast::ReleaseSavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Other(Box::new(
            crate::parser::ast::Stmt::ReleaseSavepoint(stmt.clone()),
        )))
    }

    pub(crate) fn bind_assign_variable(
        &mut self,
        stmt: &crate::parser::ast::AssignVariableStmt,
    ) -> DBResult<BoundStatement> {
        let bound_expr = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::AssignVariable(BoundAssignVariable {
            name: stmt.name.clone(),
            expression: bound_expr,
        }))
    }

    pub(crate) fn bind_filter(
        &mut self,
        stmt: &crate::parser::ast::FilterStmt,
    ) -> DBResult<BoundStatement> {
        let condition = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::Filter(BoundFilter { condition }))
    }

    pub(crate) fn bind_yield(
        &mut self,
        stmt: &crate::parser::ast::YieldStmt,
    ) -> DBResult<BoundStatement> {
        let items = stmt
            .items
            .iter()
            .map(|item| {
                self.bind_expr(&item.expression)
                    .map(|be| BoundYieldItem {
                        expression: be,
                        alias: item.alias.clone(),
                    })
            })
            .collect::<DBResult<Vec<_>>>()?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|w| self.bind_expr(w))
            .transpose()?;

        let order_by = stmt
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundStatement::Yield(BoundYield {
            items,
            where_clause,
            distinct: stmt.distinct,
            order_by,
            skip: stmt.skip.clone(),
            limit: stmt.limit.clone(),
        }))
    }

    pub(crate) fn bind_collect(
        &mut self,
        stmt: &crate::parser::ast::CollectStmt,
    ) -> DBResult<BoundStatement> {
        let items = stmt
            .items
            .iter()
            .map(|item| {
                self.bind_expr(&item.expression)
                    .map(|be| BoundYieldItem {
                        expression: be,
                        alias: item.alias.clone(),
                    })
            })
            .collect::<DBResult<Vec<_>>>()?;
        Ok(BoundStatement::Collect(BoundCollect { items }))
    }
}

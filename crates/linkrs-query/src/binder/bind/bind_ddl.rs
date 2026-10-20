use linkrs_core::DBResult;

use crate::binder::bound::*;

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
                // Cypher-style data creation is parsed but has no executable
                // write path (no id generation, no schema inference): fail
                // here instead of planning a statement that cannot run.
                // Node/edge writes are only supported through INSERT VERTEX /
                // INSERT EDGE (see docs/release/02_dml_data_manipulation.md
                // section 1 and section 2); no automatic schema creation is
                // performed for CREATE data patterns.
                Err(linkrs_core::error::DBError::from(
                    linkrs_core::error::QueryError::invalid_query(
                        "CREATE data patterns are not supported; use INSERT VERTEX / INSERT EDGE (see docs/release/02_dml_data_manipulation.md)"
                            .to_string(),
                    ),
                ))
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
        Ok(BoundStatement::Desc(BoundDesc {
            target: stmt.target.clone(),
        }))
    }

    pub(crate) fn bind_show_create(
        &mut self,
        stmt: &crate::parser::ast::ShowCreateStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ShowCreate(BoundShowCreate {
            target: stmt.target.clone(),
        }))
    }

    pub(crate) fn bind_clear_space(
        &mut self,
        stmt: &crate::parser::ast::ClearSpaceStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ClearSpace(BoundClearSpace {
            space_name: stmt.space_name.clone(),
        }))
    }

    pub(crate) fn bind_use(
        &mut self,
        stmt: &crate::parser::ast::UseStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Use(BoundUse {
            space: stmt.space.clone(),
        }))
    }

    pub(crate) fn bind_show(
        &mut self,
        stmt: &crate::parser::ast::ShowStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::Show(BoundShow {
            target: stmt.target.clone(),
        }))
    }

    pub(crate) fn bind_create_fulltext_index(
        &mut self,
        stmt: &crate::parser::ast::fulltext::CreateFulltextIndex,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::CreateFulltextIndex(
            BoundCreateFulltextIndex {
                index_name: stmt.index_name.clone(),
                schema_name: stmt.schema_name.clone(),
                fields: stmt.fields.clone(),
                engine_type: stmt.engine_type,
                options: stmt.options.clone(),
                if_not_exists: stmt.if_not_exists,
            },
        ))
    }

    pub(crate) fn bind_create_vector_index(
        &mut self,
        stmt: &crate::parser::ast::vector::CreateVectorIndex,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::CreateVectorIndex(BoundCreateVectorIndex {
            index_name: stmt.index_name.clone(),
            schema_name: stmt.schema_name.clone(),
            field_name: stmt.field_name.clone(),
            config: stmt.config.clone(),
            if_not_exists: stmt.if_not_exists,
        }))
    }

    pub(crate) fn bind_explain(
        &mut self,
        stmt: &crate::parser::ast::ExplainStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Explain(BoundExplain {
            statement: Box::new(inner),
            format: stmt.format.clone(),
            analyze: stmt.analyze,
        }))
    }

    pub(crate) fn bind_profile(
        &mut self,
        stmt: &crate::parser::ast::ProfileStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Profile(BoundProfile {
            statement: Box::new(inner),
            format: stmt.format.clone(),
        }))
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
        Ok(BoundStatement::Savepoint(BoundSavepoint {
            name: stmt.name.clone(),
        }))
    }

    pub(crate) fn bind_release_savepoint(
        &mut self,
        stmt: &crate::parser::ast::ReleaseSavepointStmt,
    ) -> DBResult<BoundStatement> {
        Ok(BoundStatement::ReleaseSavepoint(BoundReleaseSavepoint {
            name: stmt.name.clone(),
        }))
    }

    pub(crate) fn bind_assignment_statement(
        &mut self,
        stmt: &crate::parser::ast::AssignmentStmt,
    ) -> DBResult<BoundStatement> {
        let inner = self.bind_stmt(&stmt.statement)?;
        Ok(BoundStatement::Assignment(BoundAssignmentStatement {
            variable: stmt.variable.clone(),
            statement: Box::new(inner),
        }))
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
                    .map(|be| BoundProjectionItem {
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
                        self.bind_expr(&item.expression).map(|be| BoundOrderByItem {
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
                    .map(|be| BoundProjectionItem {
                        expression: be,
                        alias: item.alias.clone(),
                    })
            })
            .collect::<DBResult<Vec<_>>>()?;
        Ok(BoundStatement::Collect(BoundCollect { items }))
    }
}

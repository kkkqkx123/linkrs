//! Schema and space definition binding (DDL plus space introspection).
//!
//! Transaction control lives in [`super::bind_txn`], diagnostic wrappers in
//! [`super::bind_diagnostic`], pipe-stage clauses in [`super::bind_dml`],
//! and scripting assignments in [`super::bind_script`].
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
}

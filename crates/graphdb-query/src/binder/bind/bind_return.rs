use crate::parser::ast::ReturnItem;
use graphdb_core::types::semantic::AliasType;
use graphdb_core::DBResult;

use crate::binder::bound::*;
use crate::binder::scope::BinderVariable;

use super::Binder;

impl Binder {
    pub(crate) fn bind_return(
        &mut self,
        stmt: &crate::parser::ast::ReturnStmt,
    ) -> DBResult<BoundStatement> {
        let items = stmt
            .items
            .iter()
            .map(|item| match item {
                ReturnItem::Expression { expression, alias } => {
                    self.bind_expr(expression).map(|be| BoundReturnItem {
                        expression: be,
                        alias: alias.clone(),
                    })
                }
            })
            .collect::<DBResult<Vec<_>>>()?;

        let order_by = stmt
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| super::super::bound::BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundStatement::Return(BoundReturnStatement {
            span: stmt.span,
            items,
            distinct: stmt.distinct,
            order_by,
            skip: stmt.skip.clone(),
            limit: stmt.limit.clone(),
        }))
    }

    pub(crate) fn bind_with(
        &mut self,
        stmt: &crate::parser::ast::WithStmt,
    ) -> DBResult<BoundStatement> {
        let items = stmt
            .items
            .iter()
            .map(|item| match item {
                ReturnItem::Expression { expression, alias } => {
                    self.bind_expr(expression).map(|be| BoundReturnItem {
                        expression: be,
                        alias: alias.clone(),
                    })
                }
            })
            .collect::<DBResult<Vec<_>>>()?;

        // Register WITH aliases in scope so the WITH condition and subsequent
        // clauses can reference them.
        for item in &items {
            if let Some(alias) = &item.alias {
                self.scope.define_variable(BinderVariable {
                    name: alias.clone(),
                    alias_type: AliasType::Expression,
                    tags: Vec::new(),
                    properties: std::collections::HashMap::new(),
                    is_defined: true,
                });
            }
        }

        let condition = stmt
            .where_clause
            .as_ref()
            .map(|c| self.bind_expr(c))
            .transpose()?;

        Ok(BoundStatement::With(BoundWithStatement {
            span: stmt.span,
            items,
            condition,
        }))
    }

    pub(crate) fn bind_unwind(
        &mut self,
        stmt: &crate::parser::ast::UnwindStmt,
    ) -> DBResult<BoundStatement> {
        let expr = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::Unwind(BoundUnwindStatement {
            span: stmt.span,
            expression: expr,
            alias: stmt.variable.clone(),
        }))
    }
}

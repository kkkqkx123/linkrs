use crate::parser::ast::ReturnItem;
use linkrs_core::types::semantic::AliasType;
use linkrs_core::DBResult;

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
                    self.bind_projection_expression(expression, alias.clone(), false)
                }
            })
            .collect::<DBResult<Vec<_>>>()?;

        let order_by = self.bind_order_by_clause(stmt.order_by.as_ref())?;

        Ok(BoundStatement::Return(BoundReturnStatement {
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
                    self.bind_projection_expression(expression, alias.clone(), false)
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

        let ctes = self.bind_cte_defs(stmt)?;

        Ok(BoundStatement::With(BoundWithStatement {
            items,
            condition,
            ctes,
        }))
    }

    /// Bind the CTE definitions of a WITH statement.
    ///
    /// Under `WITH RECURSIVE` each CTE body must be `anchor UNION ALL step`;
    /// otherwise the body binds as a plain inline view. The anchor never sees
    /// its own CTE name; the step binds with the name visible so patterns can
    /// scan the working table. V1 supports exactly one CTE per WITH.
    pub(crate) fn bind_cte_defs(
        &mut self,
        stmt: &crate::parser::ast::WithStmt,
    ) -> DBResult<Vec<crate::binder::bound::BoundCteDef>> {
        use crate::parser::ast::stmt::SetOperationType;

        if stmt.recursive && stmt.ctes.is_empty() {
            return Err(crate::binder::error::invalid_query(
                "WITH RECURSIVE requires at least one CTE (`name AS (anchor UNION ALL step)`)"
                    .to_string(),
            ));
        }
        if stmt.ctes.len() > 1 {
            return Err(crate::binder::error::invalid_query(
                "Only a single CTE per WITH statement is supported".to_string(),
            ));
        }
        let mut bound = Vec::with_capacity(stmt.ctes.len());
        for cte in &stmt.ctes {
            if cte.name.trim().is_empty() {
                return Err(crate::binder::error::invalid_query(
                    "CTE name must not be empty".to_string(),
                ));
            }
            if stmt.recursive {
                let (anchor_stmt, step_stmt) = match cte.body.as_ref() {
                    crate::parser::ast::Stmt::SetOperation(setop)
                        if setop.op_type == SetOperationType::UnionAll =>
                    {
                        (setop.left.as_ref(), setop.right.as_ref())
                    }
                    _ => {
                        return Err(crate::binder::error::invalid_query(format!(
                            "Recursive CTE '{}' body must be `anchor UNION ALL step`",
                            cte.name
                        )));
                    }
                };
                let anchor = self.bind_stmt(anchor_stmt)?;
                // The step observes the CTE working table through patterns
                // labeled with the CTE name (see `resolve_tags`).
                self.cte_stack.push(cte.name.clone());
                let step = self.bind_stmt(step_stmt);
                self.cte_stack.pop();
                let step = step?;
                bound.push(crate::binder::bound::BoundCteDef {
                    name: cte.name.clone(),
                    anchor: Box::new(anchor),
                    step: Some(Box::new(step)),
                });
            } else {
                let anchor = self.bind_stmt(cte.body.as_ref())?;
                bound.push(crate::binder::bound::BoundCteDef {
                    name: cte.name.clone(),
                    anchor: Box::new(anchor),
                    step: None,
                });
            }
        }
        Ok(bound)
    }

    pub(crate) fn bind_unwind(
        &mut self,
        stmt: &crate::parser::ast::UnwindStmt,
    ) -> DBResult<BoundStatement> {
        let expr = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::Unwind(BoundUnwindStatement {
            expression: expr,
            alias: stmt.variable.clone(),
        }))
    }
}

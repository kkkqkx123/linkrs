mod bind_dml;
mod bind_expressions;
mod bind_fetch;
mod bind_go;
mod bind_lookup;
mod bind_match;
mod bind_path;
mod bind_return;
mod bind_subgraph;

use std::sync::Arc;

use crate::parser::ast::stmt::Ast;
use crate::parser::ast::Stmt;
use graphdb_core::error::DBError;
use graphdb_core::metadata::SchemaManager;
use graphdb_core::types::expr::Expression;
use graphdb_core::DBResult;

use super::bound::{BoundExpression, BoundStatement};
use super::scope::BinderScope;

use crate::executor::streaming::interner::StrInterner;

/// The Binder transforms a parsed AST into a fully resolved BoundStatement.
pub struct Binder {
    scope: BinderScope,
    schema_manager: Option<Arc<SchemaManager>>,
    space_name: Option<String>,
    space_id: u64,
    interner: StrInterner,
}

impl Binder {
    pub fn new() -> Self {
        Self {
            scope: BinderScope::new(),
            schema_manager: None,
            space_name: None,
            space_id: 0,
            interner: StrInterner::new(),
        }
    }

    pub fn with_schema_manager(mut self, sm: Arc<SchemaManager>) -> Self {
        self.schema_manager = Some(sm);
        self
    }

    pub fn with_space(mut self, space_name: Option<String>, space_id: u64) -> Self {
        self.space_name = space_name;
        self.space_id = space_id;
        self
    }

    /// Bind an AST into a fully resolved BoundStatement.
    pub fn bind(
        mut self,
        ast: Arc<Ast>,
        _qctx: Arc<crate::QueryContext>,
    ) -> DBResult<BoundStatement> {
        let bound = self.bind_stmt(&ast.stmt)?;
        Ok(bound)
    }

    // ── Statement dispatch ────────────────────────────────────────────────

    fn bind_stmt(&mut self, stmt: &Stmt) -> DBResult<BoundStatement> {
        match stmt {
            Stmt::Match(m) => self.bind_match(m),
            Stmt::Go(g) => self.bind_go(g),
            Stmt::Lookup(l) => self.bind_lookup(l),
            Stmt::Fetch(f) => self.bind_fetch(f),
            Stmt::FindPath(p) => self.bind_find_path(p),
            Stmt::Subgraph(s) => self.bind_subgraph(s),
            Stmt::Return(r) => self.bind_return(r),
            Stmt::With(w) => self.bind_with(w),
            Stmt::Unwind(u) => self.bind_unwind(u),
            Stmt::Pipe(p) => self.bind_pipe(p),
            Stmt::SetOperation(s) => self.bind_set_operation(s),
            Stmt::GroupBy(g) => self.bind_group_by(g),
            _ => Ok(BoundStatement::Other(Box::new(stmt.clone()))),
        }
    }

    /// Wrap a raw expression into a contextual expression for binding.
    fn plain_expression(expr: Expression) -> graphdb_core::types::ContextualExpression {
        let ctx = Arc::new(
            graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let id = ctx.register_expression(graphdb_core::types::expr::ExpressionMeta::new(expr));
        graphdb_core::types::ContextualExpression::new(id, ctx)
    }
}

impl Default for Binder {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[allow(clippy::arc_with_non_send_sync)]
    fn test_qctx() -> Arc<crate::QueryContext> {
        Arc::new(crate::QueryContext::new(Arc::new(
            crate::QueryRequestContext {
                session_id: None,
                user_name: None,
                space_name: None,
                query: String::new(),
                parameters: std::collections::HashMap::new(),
                ..Default::default()
            },
        )))
    }

    fn bind_query(query: &str) -> DBResult<BoundStatement> {
        let mut parser = crate::parser::Parser::new(query);
        let result = parser
            .parse()
            .map_err(|e| DBError::from(graphdb_core::error::QueryError::pipeline_parse_error(e)))?;
        Binder::new()
            .with_space(None, 0)
            .bind(result.ast, test_qctx())
    }

    #[test]
    fn test_bind_exists_subquery() {
        let bound = bind_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age > 30 } RETURN t.name",
        )
        .expect("EXISTS query should bind");
        let stmt = match bound {
            BoundStatement::Match(s) => s,
            other => panic!("expected Match, got {:?}", other.kind()),
        };
        let where_clause = stmt.where_clause.expect("where clause expected");
        match where_clause.condition {
            BoundExpression::Exists { query } => {
                let sub = query.as_match().expect("subquery should be a Match");
                assert!(sub.where_clause.is_some(), "subquery WHERE must be bound");
                assert_eq!(sub.query_graph.nodes.len(), 1);
            }
            other => panic!("expected BoundExpression::Exists, got {:?}", other),
        }
    }

    #[test]
    fn test_bind_exists_bare_pattern() {
        let bound = bind_query(
            "MATCH (t:person) WHERE EXISTS { p:person-[:knows]->q:person } RETURN t.name",
        )
        .expect("bare-pattern EXISTS query should bind");
        let stmt = match bound {
            BoundStatement::Match(s) => s,
            other => panic!("expected Match, got {:?}", other.kind()),
        };
        let where_clause = stmt.where_clause.expect("where clause expected");
        match where_clause.condition {
            BoundExpression::Exists { query } => {
                let sub = query.as_match().expect("subquery should be a Match");
                assert_eq!(sub.query_graph.nodes.len(), 2, "two nodes in pattern");
                assert_eq!(sub.query_graph.edges.len(), 1, "one edge in pattern");
            }
            other => panic!("expected BoundExpression::Exists, got {:?}", other),
        }
    }

    #[test]
    fn test_bind_in_subquery() {
        let bound = bind_query(
            "MATCH (t:person) WHERE t.name IN { MATCH (p:person) RETURN p.name } RETURN t.name",
        )
        .expect("IN query should bind");
        let stmt = match bound {
            BoundStatement::Match(s) => s,
            other => panic!("expected Match, got {:?}", other.kind()),
        };
        let where_clause = stmt.where_clause.expect("where clause expected");
        match where_clause.condition {
            BoundExpression::In {
                negated, subquery, ..
            } => {
                assert!(!negated);
                assert!(subquery.as_match().is_some());
            }
            other => panic!("expected BoundExpression::In, got {:?}", other),
        }
    }

    #[test]
    fn test_bind_correlated_subquery_resolves_outer_variable() {
        // `t` is defined by the outer MATCH and referenced inside the
        // subquery WHERE; binding must succeed via the parent scope.
        let bound = bind_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.name = t.name } RETURN t.name",
        )
        .expect("correlated EXISTS query should bind");
        let stmt = match bound {
            BoundStatement::Match(s) => s,
            other => panic!("expected Match, got {:?}", other.kind()),
        };
        assert!(stmt.where_clause.is_some());
    }

    #[test]
    fn test_bind_nested_exists() {
        let bound = bind_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) \
             WHERE EXISTS { MATCH (q:person) WHERE q.age > p.age } } RETURN t.name",
        )
        .expect("nested EXISTS query should bind");
        let stmt = match bound {
            BoundStatement::Match(s) => s,
            other => panic!("expected Match, got {:?}", other.kind()),
        };
        let where_clause = stmt.where_clause.expect("where clause expected");
        match where_clause.condition {
            BoundExpression::Exists { query } => {
                let sub = query.as_match().expect("outer subquery");
                let sub_where = sub.where_clause.as_ref().expect("inner WHERE");
                assert!(matches!(
                    sub_where.condition,
                    BoundExpression::Exists { .. }
                ));
            }
            other => panic!("expected BoundExpression::Exists, got {:?}", other),
        }
    }
}

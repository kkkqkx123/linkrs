//! Tool functions and auxiliary features

use super::stmt::*;
use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
use crate::core::Value;
use crate::query::validator::context::ExpressionAnalysisContext;
use std::sync::Arc;

/// Expression Factory – Used for creating expression nodes
pub struct ExprFactory;

impl ExprFactory {
    /// Create a constant expression.
    pub fn constant(value: Value, ctx: Arc<ExpressionAnalysisContext>) -> ContextualExpression {
        let expr = Expression::Literal(value);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a variable expression.
    pub fn variable(name: String, ctx: Arc<ExpressionAnalysisContext>) -> ContextualExpression {
        let expr = Expression::Variable(name);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a binary expression
    pub fn binary(
        left: ContextualExpression,
        op: crate::core::types::operators::BinaryOperator,
        right: ContextualExpression,
    ) -> ContextualExpression {
        let ctx = left.context().clone();
        let left_expr = left
            .expression()
            .expect("Left expression should exist")
            .expression()
            .clone();
        let right_expr = right
            .expression()
            .expect("Right expression should exist")
            .expression()
            .clone();
        let expr = Expression::Binary {
            left: Box::new(left_expr),
            op,
            right: Box::new(right_expr),
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a monomial expression.
    pub fn unary(
        op: crate::core::types::operators::UnaryOperator,
        operand: ContextualExpression,
    ) -> ContextualExpression {
        let ctx = operand.context().clone();
        let operand_expr = operand
            .expression()
            .expect("Operand expression should exist")
            .expression()
            .clone();
        let expr = Expression::Unary {
            op,
            operand: Box::new(operand_expr),
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a function call expression.
    pub fn function_call(
        name: String,
        args: Vec<ContextualExpression>,
        _distinct: bool,
    ) -> ContextualExpression {
        let ctx = if args.is_empty() {
            Arc::new(ExpressionAnalysisContext::new())
        } else {
            args[0].context().clone()
        };
        let arg_exprs: Vec<Expression> = args
            .iter()
            .map(|arg| {
                arg.expression()
                    .expect("Arg expression should exist")
                    .expression()
                    .clone()
            })
            .collect();
        let expr = Expression::Function {
            name,
            args: arg_exprs,
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Creating attribute access expressions
    pub fn property_access(object: ContextualExpression, property: String) -> ContextualExpression {
        let ctx = object.context().clone();
        let object_expr = object
            .expression()
            .expect("Object expression should exist")
            .expression()
            .clone();
        let expr = Expression::Property {
            object: Box::new(object_expr),
            property,
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a list expression.
    pub fn list(elements: Vec<ContextualExpression>) -> ContextualExpression {
        let ctx = if elements.is_empty() {
            Arc::new(ExpressionAnalysisContext::new())
        } else {
            elements[0].context().clone()
        };
        let element_exprs: Vec<Expression> = elements
            .iter()
            .map(|elem| {
                elem.expression()
                    .expect("Element expression should exist")
                    .expression()
                    .clone()
            })
            .collect();
        let expr = Expression::List(element_exprs);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a mapping expression.
    pub fn map(pairs: Vec<(String, ContextualExpression)>) -> ContextualExpression {
        let ctx = if pairs.is_empty() {
            Arc::new(ExpressionAnalysisContext::new())
        } else {
            pairs[0].1.context().clone()
        };
        let value_exprs: Vec<(String, Expression)> = pairs
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .expression()
                        .expect("Value expression should exist")
                        .expression()
                        .clone(),
                )
            })
            .collect();
        let expr = Expression::Map(value_exprs);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Creating a CASE expression
    pub fn case(
        match_expression: Option<ContextualExpression>,
        when_then_pairs: Vec<(ContextualExpression, ContextualExpression)>,
        default: Option<ContextualExpression>,
    ) -> ContextualExpression {
        let ctx = match_expression
            .as_ref()
            .map(|e| e.context().clone())
            .or_else(|| when_then_pairs.first().map(|(w, _)| w.context().clone()))
            .or_else(|| default.as_ref().map(|d| d.context().clone()))
            .unwrap_or_else(|| Arc::new(ExpressionAnalysisContext::new()));

        let test_expr = match_expression.map(|e| {
            Box::new(
                e.expression()
                    .expect("Match expression should exist")
                    .expression()
                    .clone(),
            )
        });
        let conditions = when_then_pairs
            .iter()
            .map(|(when, then)| {
                let when_expr = when
                    .expression()
                    .expect("When expression should exist")
                    .expression()
                    .clone();
                let then_expr = then
                    .expression()
                    .expect("Then expression should exist")
                    .expression()
                    .clone();
                (when_expr, then_expr)
            })
            .collect();
        let default_expr = default.map(|d| {
            Box::new(
                d.expression()
                    .expect("Default expression should exist")
                    .expression()
                    .clone(),
            )
        });
        let expr = Expression::Case {
            test_expr,
            conditions,
            default: default_expr,
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create an subscript expression.
    pub fn subscript(
        collection: ContextualExpression,
        index: ContextualExpression,
    ) -> ContextualExpression {
        let ctx = collection.context().clone();
        let collection_expr = collection
            .expression()
            .expect("Collection expression should exist")
            .expression()
            .clone();
        let index_expr = index
            .expression()
            .expect("Index expression should exist")
            .expression()
            .clone();
        let expr = Expression::Subscript {
            collection: Box::new(collection_expr),
            index: Box::new(index_expr),
        };
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);
        ContextualExpression::new(id, ctx)
    }

    /// Create a comparative expression
    pub fn compare(
        left: ContextualExpression,
        op: crate::core::types::operators::BinaryOperator,
        right: ContextualExpression,
    ) -> ContextualExpression {
        Self::binary(left, op, right)
    }

    /// Create a logical expression
    pub fn logical(
        left: ContextualExpression,
        op: crate::core::types::operators::BinaryOperator,
        right: ContextualExpression,
    ) -> ContextualExpression {
        Self::binary(left, op, right)
    }

    /// Create arithmetic expressions.
    pub fn arithmetic(
        left: ContextualExpression,
        op: crate::core::types::operators::BinaryOperator,
        right: ContextualExpression,
    ) -> ContextualExpression {
        Self::binary(left, op, right)
    }
}

/// Statement Factory – Used for creating statement nodes
pub struct StmtFactory;

impl StmtFactory {
    /// Create a query statement.
    pub fn query(statements: Vec<Stmt>, span: Span) -> Stmt {
        Stmt::Query(QueryStmt::new(statements, span))
    }

    /// Create the CREATE node statement.
    pub fn create_node(
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<ContextualExpression>,
        span: Span,
    ) -> Stmt {
        Stmt::Create(CreateStmt {
            span,
            target: CreateTarget::Node {
                variable,
                labels,
                properties,
            },
            if_not_exists: false,
        })
    }

    /// Create a CREATE edge statement.
    pub fn create_edge(
        variable: Option<String>,
        edge_type: String,
        src: ContextualExpression,
        dst: ContextualExpression,
        properties: Option<ContextualExpression>,
        direction: EdgeDirection,
        span: Span,
    ) -> Stmt {
        Stmt::Create(CreateStmt {
            span,
            target: CreateTarget::Edge {
                variable,
                edge_type,
                src,
                dst,
                properties,
                direction,
            },
            if_not_exists: false,
        })
    }

    /// Create a MATCH statement
    pub fn match_stmt(
        patterns: Vec<Pattern>,
        where_clause: Option<ContextualExpression>,
        return_clause: Option<ReturnClause>,
        order_by: Option<OrderByClause>,
        limit: Option<usize>,
        skip: Option<usize>,
        span: Span,
    ) -> Stmt {
        Stmt::Match(MatchStmt {
            span,
            patterns,
            where_clause,
            return_clause,
            order_by,
            limit,
            skip,
            optional: false,
            delete_clause: None,
        })
    }

    /// Create a DELETE statement
    pub fn delete(
        target: DeleteTarget,
        where_clause: Option<ContextualExpression>,
        span: Span,
    ) -> Stmt {
        Stmt::Delete(DeleteStmt {
            span,
            target,
            where_clause,
            with_edge: false,
        })
    }

    /// Create a DELETE statement with the WITH EDGE option
    pub fn delete_with_edge(
        target: DeleteTarget,
        where_clause: Option<ContextualExpression>,
        span: Span,
    ) -> Stmt {
        Stmt::Delete(DeleteStmt {
            span,
            target,
            where_clause,
            with_edge: true,
        })
    }

    /// Create an UPDATE statement
    pub fn update(
        target: UpdateTarget,
        set_clause: SetClause,
        where_clause: Option<ContextualExpression>,
        span: Span,
    ) -> Stmt {
        Stmt::Update(UpdateStmt {
            span,
            target,
            set_clause,
            where_clause,
            is_upsert: false,
            yield_clause: None,
        })
    }

    /// Create GO statements
    pub fn go(
        steps: Steps,
        from: FromClause,
        over: Option<OverClause>,
        where_clause: Option<ContextualExpression>,
        yield_clause: Option<YieldClause>,
        span: Span,
    ) -> Stmt {
        Stmt::Go(GoStmt {
            span,
            steps,
            from,
            over,
            where_clause,
            yield_clause,
        })
    }

    /// Create a FETCH statement
    pub fn fetch(target: FetchTarget, span: Span) -> Stmt {
        Stmt::Fetch(FetchStmt { span, target })
    }

    /// Create a USE statement
    pub fn r#use(space: String, span: Span) -> Stmt {
        Stmt::Use(UseStmt { span, space })
    }

    /// Creating a SHOW statement
    pub fn show(target: ShowTarget, span: Span) -> Stmt {
        Stmt::Show(ShowStmt { span, target })
    }

    /// Creating an EXPLAIN statement
    pub fn explain(statement: Box<Stmt>, span: Span) -> Stmt {
        Stmt::Explain(ExplainStmt {
            span,
            statement,
            format: ExplainFormat::default(),
        })
    }

    /// Creating a formatted EXPLAIN statement
    pub fn explain_with_format(statement: Box<Stmt>, format: ExplainFormat, span: Span) -> Stmt {
        Stmt::Explain(ExplainStmt {
            span,
            statement,
            format,
        })
    }

    /// Create a PROFILE statement
    pub fn profile(statement: Box<Stmt>, span: Span) -> Stmt {
        Stmt::Profile(ProfileStmt {
            span,
            statement,
            format: ExplainFormat::default(),
        })
    }

    /// Create a formatted PROFILE statement.
    pub fn profile_with_format(statement: Box<Stmt>, format: ExplainFormat, span: Span) -> Stmt {
        Stmt::Profile(ProfileStmt {
            span,
            statement,
            format,
        })
    }

    /// Create a LOOKUP statement
    pub fn lookup(
        target: LookupTarget,
        where_clause: Option<ContextualExpression>,
        yield_clause: Option<YieldClause>,
        span: Span,
    ) -> Stmt {
        Stmt::Lookup(LookupStmt {
            span,
            target,
            where_clause,
            yield_clause,
        })
    }

    /// Create the SUBGRAPH statement
    pub fn subgraph(
        steps: Steps,
        from: FromClause,
        over: Option<OverClause>,
        where_clause: Option<ContextualExpression>,
        yield_clause: Option<YieldClause>,
        span: Span,
    ) -> Stmt {
        Stmt::Subgraph(SubgraphStmt {
            span,
            steps,
            from,
            over,
            where_clause,
            yield_clause,
        })
    }

    /// Create the FIND PATH statement
    pub fn find_path(
        from: FromClause,
        to: ContextualExpression,
        over: Option<OverClause>,
        where_clause: Option<ContextualExpression>,
        shortest: bool,
        yield_clause: Option<YieldClause>,
        span: Span,
    ) -> Stmt {
        Stmt::FindPath(FindPathStmt {
            span,
            from,
            to,
            over,
            where_clause,
            shortest,
            max_steps: None,
            limit: None,
            offset: None,
            yield_clause,
            weight_expression: None,
            heuristic_expression: None,
            with_loop: false,
            with_cycle: false,
        })
    }
}

/// Pattern Factory – Used for creating pattern nodes
pub struct PatternFactory;

impl PatternFactory {
    /// Create a node pattern.
    pub fn node(
        variable: Option<String>,
        labels: Vec<String>,
        properties: Option<ContextualExpression>,
        predicates: Vec<ContextualExpression>,
        span: Span,
    ) -> Pattern {
        Pattern::Node(NodePattern::new(
            variable, labels, properties, predicates, span,
        ))
    }

    /// Create a edge pattern.
    pub fn edge(
        variable: Option<String>,
        edge_types: Vec<String>,
        properties: Option<ContextualExpression>,
        predicates: Vec<ContextualExpression>,
        direction: EdgeDirection,
        range: Option<EdgeRange>,
        span: Span,
    ) -> Pattern {
        Pattern::Edge(EdgePattern::new(
            variable, edge_types, properties, predicates, direction, range, span,
        ))
    }

    /// Create a path pattern
    pub fn path(elements: Vec<PathElement>, span: Span) -> Pattern {
        Pattern::Path(PathPattern::new(elements, span))
    }

    /// Create a variable pattern.
    pub fn variable(name: String, span: Span) -> Pattern {
        Pattern::Variable(VariablePattern::new(name, span))
    }

    /// Create a simple node pattern.
    pub fn simple_node(variable: Option<String>, labels: Vec<String>, span: Span) -> Pattern {
        Self::node(variable, labels, None, vec![], span)
    }

    /// Create a simple edge pattern.
    pub fn simple_edge(
        variable: Option<String>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        span: Span,
    ) -> Pattern {
        Self::edge(variable, edge_types, None, vec![], direction, None, span)
    }

    /// Create a directed edge pattern
    pub fn directed_edge(
        variable: Option<String>,
        edge_type: String,
        direction: EdgeDirection,
        span: Span,
    ) -> Pattern {
        Self::simple_edge(variable, vec![edge_type], direction, span)
    }

    /// Create an undirected edge pattern.
    pub fn undirected_edge(variable: Option<String>, edge_type: String, span: Span) -> Pattern {
        Self::simple_edge(variable, vec![edge_type], EdgeDirection::Both, span)
    }
}

/// The AST builder is used for constructing complex AST (Abstract Syntax Tree) structures.
pub struct AstBuilder {
    span: Span,
}

impl AstBuilder {
    pub fn new(span: Span) -> Self {
        Self { span }
    }

    /// Constructing a simple MATCH query
    pub fn build_simple_match(
        &self,
        pattern: Pattern,
        return_expression: ContextualExpression,
    ) -> Stmt {
        let return_clause = ReturnClause {
            span: self.span,
            items: vec![ReturnItem::Expression {
                expression: return_expression,
                alias: None,
            }],
            distinct: false,
            order_by: None,
            limit: None,
            skip: None,
            sample: None,
            having_clause: None,
        };

        StmtFactory::match_stmt(
            vec![pattern],
            None,
            Some(return_clause),
            None,
            None,
            None,
            self.span,
        )
    }

    /// Constructing a simple CREATE node query
    pub fn build_create_node(&self, variable: Option<String>, labels: Vec<String>) -> Stmt {
        StmtFactory::create_node(variable, labels, None, self.span)
    }

    /// Constructing a simple CREATE edge query
    pub fn build_create_edge(
        &self,
        variable: Option<String>,
        edge_type: String,
        src: ContextualExpression,
        dst: ContextualExpression,
        direction: EdgeDirection,
    ) -> Stmt {
        StmtFactory::create_edge(variable, edge_type, src, dst, None, direction, self.span)
    }

    /// Constructing a simple DELETE query
    pub fn build_delete_vertices(&self, vertices: Vec<ContextualExpression>) -> Stmt {
        StmtFactory::delete(DeleteTarget::Vertices(vertices), None, self.span)
    }

    /// Constructing a simple UPDATE query
    pub fn build_update_vertex(
        &self,
        vertex: ContextualExpression,
        assignments: Vec<Assignment>,
    ) -> Stmt {
        let set_clause = SetClause {
            span: self.span,
            assignments,
        };

        StmtFactory::update(UpdateTarget::Vertex(vertex), set_clause, None, self.span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use std::sync::Arc;

    #[test]
    fn test_expr_factory() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());

        // Testing constant expressions
        let const_expression = ExprFactory::constant(Value::Int(42), ctx.clone());
        assert!(const_expression.expression().is_some());

        // Test variable expressions
        let var_expression = ExprFactory::variable("x".to_string(), ctx.clone());
        assert!(var_expression.expression().is_some());

        // Testing binary expressions
        let left = ExprFactory::constant(Value::Int(5), ctx.clone());
        let right = ExprFactory::constant(Value::Int(3), ctx.clone());
        let binary_expression = ExprFactory::binary(
            left,
            crate::core::types::operators::BinaryOperator::Add,
            right,
        );
        assert!(binary_expression.expression().is_some());
    }
}

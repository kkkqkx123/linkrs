use std::sync::Arc;

use crate::core::error::DBError;
use crate::core::metadata::SchemaManager;
use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::semantic::{AliasType, ValueType};
use crate::core::types::EdgeDirection;
use crate::core::value::NullType;
use crate::core::DBResult;
use crate::core::DataType;
use crate::core::Value;
use crate::query::parser::ast::pattern::{PathElement, Pattern};
use crate::query::parser::ast::stmt::Ast;
use crate::query::parser::ast::{
    FetchTarget, MatchDeleteTarget, ReturnItem, SetOperationType, Stmt,
};

use super::bound::{
    BoundAggregateCall, BoundExpression, BoundFetchEdgesStatement, BoundFetchVerticesStatement,
    BoundFindPathStatement, BoundFunctionCall, BoundGoStatement, BoundGroupByStatement,
    BoundLookupStatement, BoundLookupTarget, BoundMatchDeleteClause, BoundMatchDeleteTarget,
    BoundMatchStatement, BoundPipeStatement, BoundReturnClause, BoundReturnItem,
    BoundReturnStatement, BoundSetOperationStatement, BoundStatement, BoundSubgraphStatement,
    BoundUnwindStatement, BoundWhereClause, BoundWithStatement, BoundYieldClause, BoundYieldItem,
    SetOperationKind,
};
use super::expr_binder::ExpressionBinder;
use super::query_graph::{
    BoundEdgePattern, BoundEdgeTypeRef, BoundNodePattern, BoundTagRef, QueryGraph,
};
use super::scope::{BinderScope, BinderVariable};

use crate::query::executor::streaming::interner::StrInterner;

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
        _qctx: Arc<crate::query::QueryContext>,
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

    // ── Expression binding ─────────────────────────────────────────────────

    fn bind_expr(&mut self, expr: &ContextualExpression) -> DBResult<BoundExpression> {
        super::semantic_checker::validate_expression(expr)?;
        let type_hint = expr.data_type();
        let Some(inner) = expr.get_expression() else {
            return Err(DBError::from(
                crate::core::error::QueryError::invalid_query(
                    "Expression not found in context".to_string(),
                ),
            ));
        };
        self.bind_inner_expr(&inner, type_hint.as_ref())
    }

    fn bind_inner_expr(
        &mut self,
        expr: &Expression,
        type_hint: Option<&DataType>,
    ) -> DBResult<BoundExpression> {
        match expr {
            Expression::Literal(v) => {
                let dt = v.data_type();
                Ok(BoundExpression::Literal(v.clone(), dt))
            }
            Expression::Variable(v) => {
                let dt = type_hint.cloned().unwrap_or(DataType::String);
                let _col_type = if let Some(var_info) = self.scope.lookup(v) {
                    var_info.properties.get(v).cloned()
                } else {
                    None
                };
                Ok(BoundExpression::Variable(v.clone(), dt))
            }
            Expression::Property { object, property } => {
                let obj = self.bind_inner_expr(object, type_hint)?;
                let var_name = match object.as_ref() {
                    Expression::Variable(v) => Some(v.clone()),
                    _ => None,
                };
                let prop_type = if let Some(ref var_name) = var_name {
                    self.resolve_property_type(var_name, property)
                } else {
                    DataType::String
                };
                Ok(BoundExpression::Property {
                    object: Box::new(obj),
                    property: property.clone(),
                    value_type: prop_type,
                })
            }
            Expression::Binary { left, op, right } => {
                let left = self.bind_inner_expr(left, None)?;
                let right = self.bind_inner_expr(right, None)?;
                let return_type = type_hint.cloned().unwrap_or_else(|| {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    expr_binder.deduce_arithmetic_type(&left.return_type(), &right.return_type())
                });
                Ok(BoundExpression::BinaryOp {
                    left: Box::new(left),
                    op: *op,
                    right: Box::new(right),
                    return_type,
                })
            }
            Expression::Unary { op, operand } => {
                let operand = self.bind_inner_expr(operand, None)?;
                let return_type = type_hint.cloned().unwrap_or(DataType::Bool);
                Ok(BoundExpression::UnaryOp {
                    op: *op,
                    operand: Box::new(operand),
                    return_type,
                })
            }
            Expression::Function { name, args } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let arg_types: Vec<DataType> = args.iter().map(|a| a.return_type()).collect();
                let return_type = {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    ValueType::from_data_type(
                        &expr_binder.deduce_function_return_type(name, &arg_types),
                    )
                };
                Ok(BoundExpression::Function(BoundFunctionCall {
                    name: name.clone(),
                    args,
                    return_type,
                }))
            }
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter: _filter,
            } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let arg_type = args
                    .first()
                    .map(|a| a.return_type())
                    .unwrap_or(DataType::Empty);
                let return_type = {
                    let expr_binder = ExpressionBinder::new(&self.scope);
                    ValueType::from_data_type(
                        &expr_binder.deduce_aggregate_return_type(func, &arg_type),
                    )
                };
                Ok(BoundExpression::Aggregate(BoundAggregateCall {
                    function_name: format!("{:?}", func),
                    arguments: args,
                    distinct: *distinct,
                    alias: None,
                    return_type,
                }))
            }
            Expression::List(items) => {
                let items = items
                    .iter()
                    .map(|i| self.bind_inner_expr(i, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::List(items, DataType::List))
            }
            Expression::Map(entries) => {
                let entries = entries
                    .iter()
                    .map(|(k, v)| self.bind_inner_expr(v, None).map(|b| (k.clone(), b)))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Map(entries, DataType::Map))
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let test = test_expr
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let conds = conditions
                    .iter()
                    .map(|(c, r)| {
                        self.bind_inner_expr(c, None)
                            .and_then(|bc| self.bind_inner_expr(r, None).map(|br| (bc, br)))
                    })
                    .collect::<DBResult<Vec<_>>>()?;
                let def = default
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let return_type = conds
                    .first()
                    .map(|(_, v)| v.return_type())
                    .or_else(|| def.as_ref().map(|d| d.return_type()))
                    .unwrap_or(DataType::String);
                Ok(BoundExpression::Case {
                    expr: test.map(Box::new),
                    when_then: conds,
                    else_expr: def.map(Box::new),
                    return_type,
                })
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let e = self.bind_inner_expr(expression, Some(target_type))?;
                Ok(BoundExpression::Cast {
                    expr: Box::new(e),
                    target_type: target_type.clone(),
                })
            }
            Expression::Subscript { collection, index } => {
                let col = self.bind_inner_expr(collection, None)?;
                let idx = self.bind_inner_expr(index, None)?;
                Ok(BoundExpression::Subscript {
                    collection: Box::new(col),
                    index: Box::new(idx),
                    return_type: DataType::String,
                })
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let col = self.bind_inner_expr(collection, None)?;
                let s = start
                    .as_ref()
                    .map(|e| self.bind_inner_expr(e, None))
                    .transpose()?;
                let e = end
                    .as_ref()
                    .map(|r| self.bind_inner_expr(r, None))
                    .transpose()?;
                Ok(BoundExpression::List(
                    vec![
                        col,
                        s.unwrap_or(BoundExpression::Literal(
                            Value::Null(NullType::Null),
                            DataType::Null,
                        )),
                        e.unwrap_or(BoundExpression::Literal(
                            Value::Null(NullType::Null),
                            DataType::Null,
                        )),
                    ],
                    DataType::List,
                ))
            }
            Expression::Path(elements) => {
                let elems = elements
                    .iter()
                    .map(|e| self.bind_inner_expr(e, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Path(elems, DataType::List))
            }
            Expression::Label(l) => Ok(BoundExpression::Label(l.clone())),
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let src = self.bind_inner_expr(source, None)?;
                let flt = filter
                    .as_ref()
                    .map(|f| self.bind_inner_expr(f, None))
                    .transpose()?;
                let mp = map
                    .as_ref()
                    .map(|m| self.bind_inner_expr(m, None))
                    .transpose()?;
                Ok(BoundExpression::ListComprehension {
                    variable: variable.clone(),
                    source: Box::new(src),
                    filter: flt.map(Box::new),
                    map: mp.map(Box::new),
                    return_type: DataType::List,
                })
            }
            Expression::LabelTagProperty { tag, property } => {
                let tag_name = match tag.as_ref() {
                    Expression::Variable(v) => v.clone(),
                    _ => format!("{:?}", tag),
                };
                Ok(BoundExpression::TagProperty {
                    tag_name,
                    property: property.clone(),
                    value_type: DataType::String,
                })
            }
            Expression::TagProperty { tag_name, property } => Ok(BoundExpression::TagProperty {
                tag_name: tag_name.clone(),
                property: property.clone(),
                value_type: DataType::String,
            }),
            Expression::EdgeProperty {
                edge_name,
                property,
            } => Ok(BoundExpression::EdgeProperty {
                edge_name: edge_name.clone(),
                property: property.clone(),
                value_type: DataType::String,
            }),
            Expression::Predicate { func, args } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::Predicate {
                    func: func.clone(),
                    args,
                    return_type: DataType::Bool,
                })
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                let init = self.bind_inner_expr(initial, None)?;
                let src = self.bind_inner_expr(source, None)?;
                let map = self.bind_inner_expr(mapping, None)?;
                Ok(BoundExpression::Reduce {
                    accumulator: accumulator.clone(),
                    initial: Box::new(init),
                    variable: variable.clone(),
                    source: Box::new(src),
                    mapping: Box::new(map),
                    return_type: DataType::String,
                })
            }
            Expression::PathBuild(elements) => {
                let elems = elements
                    .iter()
                    .map(|e| self.bind_inner_expr(e, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::PathBuild(elems, DataType::List))
            }
            Expression::Parameter(p) => {
                Ok(BoundExpression::ParameterRef(p.clone(), DataType::String))
            }
            Expression::Vector(v) => Ok(BoundExpression::Vector(v.clone())),
            Expression::WindowFunction {
                name,
                args,
                over_partition_by,
                over_order_by,
                over_order_desc,
            } => {
                let args = args
                    .iter()
                    .map(|a| self.bind_inner_expr(a, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let part_by = over_partition_by
                    .iter()
                    .map(|p| self.bind_inner_expr(p, None))
                    .collect::<DBResult<Vec<_>>>()?;
                let order_by = over_order_by
                    .iter()
                    .map(|o| self.bind_inner_expr(o, None))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundExpression::WindowFunction {
                    name: name.clone(),
                    args,
                    over_partition_by: part_by,
                    over_order_by: order_by,
                    over_order_desc: over_order_desc.clone(),
                    return_type: DataType::String,
                })
            }
            Expression::Exists { body: _body } => Err(DBError::from(
                crate::core::error::QueryError::invalid_query(
                    "EXISTS subquery binding not yet implemented".to_string(),
                ),
            )),
            Expression::In {
                expr: innerexpr,
                subquery: _subquery,
                negated: _negated,
            } => {
                let _e = self.bind_inner_expr(innerexpr, None)?;
                Err(DBError::from(
                    crate::core::error::QueryError::invalid_query(
                        "IN subquery binding not yet implemented".to_string(),
                    ),
                ))
            }
        }
    }

    fn resolve_property_type(&self, var_name: &str, property: &str) -> DataType {
        if let Some(var_info) = self.scope.lookup(var_name) {
            if let Some(vt) = var_info.properties.get(property) {
                return vt.to_data_type();
            }
            if let Some(ref sm) = self.schema_manager {
                if let Some(ref space_name) = self.space_name {
                    for tag_name in &var_info.tags {
                        if let Ok(Some(tag_info)) = sm.get_tag(space_name, tag_name) {
                            for prop in &tag_info.properties {
                                if prop.name == property {
                                    return prop.data_type.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
        DataType::String
    }

    // ── MATCH binding (produces QueryGraph) ────────────────────────────────

    fn bind_match(
        &mut self,
        stmt: &crate::query::parser::ast::MatchStmt,
    ) -> DBResult<BoundStatement> {
        let query_graph = self.build_query_graph(&stmt.patterns)?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let return_clause = stmt
            .return_clause
            .as_ref()
            .map(|rc| self.bind_return_clause(rc))
            .transpose()?;

        let order_by = stmt
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| super::bound::BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        let delete_clause = stmt
            .delete_clause
            .as_ref()
            .map(|dc| self.bind_match_delete(dc))
            .transpose()?;

        // Register MATCH variables in scope
        for node in &query_graph.nodes {
            self.scope.define_variable(BinderVariable {
                name: node.variable.clone(),
                alias_type: AliasType::Node,
                tags: node.tags.iter().map(|t| t.tag_name.to_string()).collect(),
                properties: node
                    .tags
                    .iter()
                    .flat_map(|t| t.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }
        for edge in &query_graph.edges {
            self.scope.define_variable(BinderVariable {
                name: edge.variable.clone(),
                alias_type: AliasType::Edge,
                tags: edge
                    .edge_types
                    .iter()
                    .map(|e| e.edge_type_name.to_string())
                    .collect(),
                properties: edge
                    .edge_types
                    .iter()
                    .flat_map(|e| e.properties.clone())
                    .collect(),
                is_defined: true,
            });
        }

        Ok(BoundStatement::Match(BoundMatchStatement {
            span: stmt.span,
            query_graph,
            where_clause,
            return_clause,
            order_by,
            limit: stmt.limit,
            skip: stmt.skip,
            optional: stmt.optional,
            delete_clause,
        }))
    }

    fn build_query_graph(&mut self, patterns: &[Pattern]) -> DBResult<QueryGraph> {
        let mut graph = QueryGraph::new();

        for pattern in patterns {
            self.process_pattern(pattern, &mut graph, None)?;
        }

        Ok(graph)
    }

    fn process_pattern(
        &mut self,
        pattern: &Pattern,
        graph: &mut QueryGraph,
        prev_node_var: Option<String>,
    ) -> DBResult<Option<String>> {
        match pattern {
            Pattern::Node(np) => {
                let var = np
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_n{}", graph.node_count()));

                let tags = self.resolve_tags(&np.labels)?;
                graph.add_node(BoundNodePattern {
                    variable: var.clone(),
                    tags,
                });
                Ok(Some(var))
            }
            Pattern::Edge(ep) => {
                let var = ep
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_e{}", graph.edge_count()));

                let edge_types = self.resolve_edge_types(&ep.edge_types)?;

                let src = prev_node_var
                    .clone()
                    .unwrap_or_else(|| format!("__src_{}", var));
                let dst = format!("__dst_{}", var);

                graph.add_edge(BoundEdgePattern {
                    variable: var,
                    edge_types,
                    direction: ep.direction,
                    src_variable: src,
                    dst_variable: dst,
                });
                Ok(None)
            }
            Pattern::Path(pp) => {
                let mut last_var = prev_node_var;
                for element in &pp.elements {
                    last_var = self.process_path_element(element, graph, last_var)?;
                }
                Ok(last_var)
            }
            Pattern::Variable(vp) => {
                if !self.scope.contains(&vp.name) {
                    return Err(DBError::from(
                        crate::core::error::QueryError::invalid_query(format!(
                            "Undefined variable: {}",
                            vp.name
                        )),
                    ));
                }
                Ok(Some(vp.name.clone()))
            }
        }
    }

    fn process_path_element(
        &mut self,
        element: &PathElement,
        graph: &mut QueryGraph,
        prev_node_var: Option<String>,
    ) -> DBResult<Option<String>> {
        match element {
            PathElement::Node(np) => {
                let var = np
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_n{}", graph.node_count()));
                let tags = self.resolve_tags(&np.labels)?;
                graph.add_node(BoundNodePattern {
                    variable: var.clone(),
                    tags,
                });
                Ok(Some(var))
            }
            PathElement::Edge(ep) => {
                let var = ep
                    .variable
                    .clone()
                    .unwrap_or_else(|| format!("__anon_e{}", graph.edge_count()));
                let edge_types = self.resolve_edge_types(&ep.edge_types)?;

                let src = prev_node_var
                    .clone()
                    .unwrap_or_else(|| format!("__src_{}", var));
                let dst = format!("__dst_{}", var);

                graph.add_edge(BoundEdgePattern {
                    variable: var,
                    edge_types,
                    direction: ep.direction,
                    src_variable: src,
                    dst_variable: dst,
                });
                Ok(None)
            }
            PathElement::Alternative(alt_patterns) => {
                for alt in alt_patterns {
                    self.process_pattern(alt, graph, prev_node_var.clone())?;
                }
                Ok(None)
            }
            PathElement::Optional(elem) => self.process_path_element(elem, graph, prev_node_var),
            PathElement::Repeated(elem, _rep) => {
                self.process_path_element(elem, graph, prev_node_var)
            }
        }
    }

    // ── Catalog resolution ─────────────────────────────────────────────────

    fn resolve_tags(&self, labels: &[String]) -> DBResult<Vec<BoundTagRef>> {
        if labels.is_empty() {
            return Ok(vec![]);
        }

        let mut resolved = Vec::new();
        if let Some(ref sm) = self.schema_manager {
            if let Some(ref space_name) = self.space_name {
                for label in labels {
                    let tag_info =
                        sm.get_tag(space_name, label)
                            .map_err(|e| {
                                DBError::from(crate::core::error::QueryError::invalid_query(
                                    format!("Failed to resolve tag '{}': {}", label, e),
                                ))
                            })?
                            .ok_or_else(|| {
                                DBError::from(crate::core::error::QueryError::invalid_query(
                                    format!("Tag '{}' not found in space '{}'", label, space_name),
                                ))
                            })?;

                    let mut properties = std::collections::HashMap::new();
                    for prop in &tag_info.properties {
                        properties.insert(
                            prop.name.clone(),
                            ValueType::from_data_type(&prop.data_type),
                        );
                    }

                    resolved.push(BoundTagRef {
                        tag_name: self.interner.intern(label),
                        properties,
                    });
                }
                return Ok(resolved);
            }
        }

        for label in labels {
            resolved.push(BoundTagRef {
                tag_name: self.interner.intern(label),
                properties: std::collections::HashMap::new(),
            });
        }
        Ok(resolved)
    }

    fn resolve_edge_types(&self, edge_types: &[String]) -> DBResult<Vec<BoundEdgeTypeRef>> {
        if edge_types.is_empty() {
            return Ok(vec![]);
        }

        let mut resolved = Vec::new();
        if let Some(ref sm) = self.schema_manager {
            if let Some(ref space_name) = self.space_name {
                for et in edge_types {
                    let edge_info = sm
                        .get_edge_type(space_name, et)
                        .map_err(|e| {
                            DBError::from(crate::core::error::QueryError::invalid_query(format!(
                                "Failed to resolve edge type '{}': {}",
                                et, e
                            )))
                        })?
                        .ok_or_else(|| {
                            DBError::from(crate::core::error::QueryError::invalid_query(format!(
                                "Edge type '{}' not found in space '{}'",
                                et, space_name
                            )))
                        })?;

                    let mut properties = std::collections::HashMap::new();
                    for prop in &edge_info.properties {
                        properties.insert(
                            prop.name.clone(),
                            ValueType::from_data_type(&prop.data_type),
                        );
                    }

                    resolved.push(BoundEdgeTypeRef {
                        edge_type_name: self.interner.intern(et),
                        properties,
                    });
                }
                return Ok(resolved);
            }
        }

        for et in edge_types {
            resolved.push(BoundEdgeTypeRef {
                edge_type_name: self.interner.intern(et),
                properties: std::collections::HashMap::new(),
            });
        }
        Ok(resolved)
    }

    // ── Other statement binders ────────────────────────────────────────────

    fn bind_go(&mut self, stmt: &crate::query::parser::ast::GoStmt) -> DBResult<BoundStatement> {
        let from = stmt
            .from
            .vertices
            .iter()
            .map(|v| self.bind_expr(v))
            .collect::<DBResult<Vec<_>>>()?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let yield_clause = stmt
            .yield_clause
            .as_ref()
            .map(|yc| self.bind_yield_clause(yc))
            .transpose()?;

        if let Some(ref o) = stmt.over {
            self.resolve_edge_types(&o.edge_types)?;
        }

        let over = stmt.over.as_ref().map(|o| o.edge_types.clone());

        Ok(BoundStatement::Go(BoundGoStatement {
            span: stmt.span,
            steps: stmt.steps.clone(),
            from,
            over,
            direction: stmt
                .over
                .as_ref()
                .map(|o| o.direction)
                .unwrap_or(EdgeDirection::Out),
            where_clause,
            yield_clause,
        }))
    }

    fn bind_lookup(
        &mut self,
        stmt: &crate::query::parser::ast::LookupStmt,
    ) -> DBResult<BoundStatement> {
        let target = match &stmt.target {
            crate::query::parser::ast::LookupTarget::Tag(t) => {
                self.resolve_tags(std::slice::from_ref(t))?;
                BoundLookupTarget::Tag(t.clone())
            }
            crate::query::parser::ast::LookupTarget::Edge(e) => {
                self.resolve_edge_types(std::slice::from_ref(e))?;
                BoundLookupTarget::Edge(e.clone())
            }
            crate::query::parser::ast::LookupTarget::Unspecified(s) => {
                let is_edge = match self.resolve_tags(std::slice::from_ref(s)) {
                    Ok(_) => false,
                    Err(_) => {
                        self.resolve_edge_types(std::slice::from_ref(s))?;
                        true
                    }
                };
                if is_edge {
                    BoundLookupTarget::Edge(s.clone())
                } else {
                    BoundLookupTarget::Tag(s.clone())
                }
            }
        };

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let yield_clause = stmt
            .yield_clause
            .as_ref()
            .map(|yc| self.bind_yield_clause(yc))
            .transpose()?;

        Ok(BoundStatement::Lookup(BoundLookupStatement {
            span: stmt.span,
            target,
            where_clause,
            yield_clause,
        }))
    }

    fn bind_fetch(
        &mut self,
        stmt: &crate::query::parser::ast::FetchStmt,
    ) -> DBResult<BoundStatement> {
        match &stmt.target {
            FetchTarget::Vertices {
                tag_name,
                ids,
                properties,
            } => {
                if let Some(tag_name) = tag_name {
                    self.resolve_tags(std::slice::from_ref(tag_name))?;
                }
                let bound_ids = ids
                    .iter()
                    .map(|id| self.bind_expr(id))
                    .collect::<DBResult<Vec<_>>>()?;
                Ok(BoundStatement::FetchVertices(BoundFetchVerticesStatement {
                    span: stmt.span,
                    tag_name: tag_name.clone(),
                    ids: bound_ids,
                    properties: properties.clone(),
                }))
            }
            FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                properties,
            } => {
                let bound_src = self.bind_expr(src)?;
                let bound_dst = self.bind_expr(dst)?;
                let bound_rank = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                Ok(BoundStatement::FetchEdges(BoundFetchEdgesStatement {
                    span: stmt.span,
                    src: bound_src,
                    dst: bound_dst,
                    edge_type: edge_type.clone(),
                    rank: bound_rank,
                    properties: properties.clone(),
                }))
            }
        }
    }

    fn bind_find_path(
        &mut self,
        stmt: &crate::query::parser::ast::FindPathStmt,
    ) -> DBResult<BoundStatement> {
        let from = stmt
            .from
            .vertices
            .iter()
            .map(|v| self.bind_expr(v))
            .collect::<DBResult<Vec<_>>>()?;
        let to = self.bind_expr(&stmt.to)?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let yield_clause = stmt
            .yield_clause
            .as_ref()
            .map(|yc| self.bind_yield_clause(yc))
            .transpose()?;

        let over = stmt
            .over
            .as_ref()
            .map(|o| (o.edge_types.clone(), o.direction));

        Ok(BoundStatement::FindPath(BoundFindPathStatement {
            span: stmt.span,
            from,
            to,
            over,
            where_clause,
            shortest: stmt.shortest,
            max_steps: stmt.max_steps,
            limit: stmt.limit,
            offset: stmt.offset,
            yield_clause,
        }))
    }

    fn bind_subgraph(
        &mut self,
        stmt: &crate::query::parser::ast::SubgraphStmt,
    ) -> DBResult<BoundStatement> {
        let from = stmt
            .from
            .vertices
            .iter()
            .map(|v| self.bind_expr(v))
            .collect::<DBResult<Vec<_>>>()?;

        let where_clause = stmt
            .where_clause
            .as_ref()
            .map(|c| {
                self.bind_expr(c)
                    .map(|be| BoundWhereClause { condition: be })
            })
            .transpose()?;

        let yield_clause = stmt
            .yield_clause
            .as_ref()
            .map(|yc| self.bind_yield_clause(yc))
            .transpose()?;

        let over = stmt
            .over
            .as_ref()
            .map(|o| (o.edge_types.clone(), o.direction));

        Ok(BoundStatement::Subgraph(BoundSubgraphStatement {
            span: stmt.span,
            steps: stmt.steps.clone(),
            from,
            over,
            where_clause,
            yield_clause,
        }))
    }

    fn bind_return(
        &mut self,
        stmt: &crate::query::parser::ast::ReturnStmt,
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
                            .map(|be| super::bound::BoundOrderByItem {
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
            skip: stmt.skip,
            limit: stmt.limit,
        }))
    }

    fn bind_with(
        &mut self,
        stmt: &crate::query::parser::ast::WithStmt,
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

    fn bind_unwind(
        &mut self,
        stmt: &crate::query::parser::ast::UnwindStmt,
    ) -> DBResult<BoundStatement> {
        let expr = self.bind_expr(&stmt.expression)?;
        Ok(BoundStatement::Unwind(BoundUnwindStatement {
            span: stmt.span,
            expression: expr,
            alias: stmt.variable.clone(),
        }))
    }

    fn bind_pipe(
        &mut self,
        stmt: &crate::query::parser::ast::PipeStmt,
    ) -> DBResult<BoundStatement> {
        let statements = vec![self.bind_stmt(&stmt.left)?, self.bind_stmt(&stmt.right)?];

        Ok(BoundStatement::Pipe(BoundPipeStatement {
            span: stmt.span,
            statements,
        }))
    }

    fn bind_set_operation(
        &mut self,
        stmt: &crate::query::parser::ast::SetOperationStmt,
    ) -> DBResult<BoundStatement> {
        let left = Box::new(self.bind_stmt(&stmt.left)?);
        let right = Box::new(self.bind_stmt(&stmt.right)?);
        let operation = match stmt.op_type {
            SetOperationType::Union | SetOperationType::UnionAll => SetOperationKind::Union,
            SetOperationType::Intersect => SetOperationKind::Intersect,
            SetOperationType::Minus => SetOperationKind::Minus,
        };
        Ok(BoundStatement::SetOperation(BoundSetOperationStatement {
            span: stmt.span,
            left,
            right,
            operation,
        }))
    }

    fn bind_group_by(
        &mut self,
        stmt: &crate::query::parser::ast::GroupByStmt,
    ) -> DBResult<BoundStatement> {
        let keys = stmt
            .group_items
            .iter()
            .map(|k| self.bind_expr(k))
            .collect::<DBResult<Vec<_>>>()?;

        Ok(BoundStatement::GroupBy(BoundGroupByStatement {
            span: stmt.span,
            keys,
            aggregates: Vec::new(),
        }))
    }

    // ── Clause helpers ─────────────────────────────────────────────────────

    fn bind_return_clause(
        &mut self,
        rc: &crate::query::parser::ast::ReturnClause,
    ) -> DBResult<BoundReturnClause> {
        let items = rc
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

        let order_by = rc
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| super::bound::BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundReturnClause {
            items,
            distinct: rc.distinct,
            order_by,
            limit: rc.limit.clone(),
            skip: rc.skip.clone(),
            sample: rc.sample.clone(),
        })
    }

    fn bind_yield_clause(
        &mut self,
        yc: &crate::query::parser::ast::YieldClause,
    ) -> DBResult<BoundYieldClause> {
        let items = yc
            .items
            .iter()
            .map(|item| {
                self.bind_expr(&item.expression).map(|be| BoundYieldItem {
                    expression: be,
                    alias: item.alias.clone(),
                })
            })
            .collect::<DBResult<Vec<_>>>()?;

        let order_by = yc
            .order_by
            .as_ref()
            .map(|ob| {
                ob.items
                    .iter()
                    .map(|item| {
                        self.bind_expr(&item.expression)
                            .map(|be| super::bound::BoundOrderByItem {
                                expression: be,
                                direction: item.direction,
                            })
                    })
                    .collect::<DBResult<Vec<_>>>()
            })
            .transpose()?;

        Ok(BoundYieldClause {
            items,
            distinct: false,
            order_by,
            limit: yc.limit.clone(),
            skip: yc.skip.clone(),
        })
    }

    fn bind_match_delete(
        &mut self,
        dc: &crate::query::parser::ast::MatchDeleteClause,
    ) -> DBResult<BoundMatchDeleteClause> {
        let target = match &dc.target {
            MatchDeleteTarget::Vertices(exprs) => {
                let bound = exprs
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundMatchDeleteTarget::Vertices(bound)
            }
            MatchDeleteTarget::Edges(exprs) => {
                let bound = exprs
                    .iter()
                    .map(|e| self.bind_expr(e))
                    .collect::<DBResult<Vec<_>>>()?;
                BoundMatchDeleteTarget::Edges(bound)
            }
            MatchDeleteTarget::EdgeRefs(refs) => {
                let mut bound = Vec::new();
                for (src, dst, rank) in refs {
                    let bsrc = self.bind_expr(src)?;
                    let bdst = self.bind_expr(dst)?;
                    let brank = rank.as_ref().map(|r| self.bind_expr(r)).transpose()?;
                    bound.push((bsrc, bdst, brank));
                }
                BoundMatchDeleteTarget::EdgeRefs(bound)
            }
        };
        Ok(BoundMatchDeleteClause {
            target,
            with_edge: dc.with_edge,
        })
    }
}

impl Default for Binder {
    fn default() -> Self {
        Self::new()
    }
}

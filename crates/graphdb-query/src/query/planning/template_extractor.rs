//! Query Template Extractor
//!
//! This module provides functions for parameterizing query requests and extracting templates, which are used for planning the caching process.
//! Replace the specific parameter values with placeholders, so that queries with semantically equivalent content can share the cache.

use crate::core::types::expr::{ContextualExpression, Expression};
use crate::core::{NullType, Value};
use crate::query::parser::ast::stmt::OrderDirection;
use crate::query::parser::ast::stmt::{
    DeleteStmt, FetchStmt, FromClause, GoStmt, InsertStmt, LookupStmt, MatchStmt, Pattern,
    ReturnClause, ReturnItem, SetClause, Stmt, UpdateStmt, YieldClause,
};

/// Parameterized results
#[derive(Debug, Clone)]
pub struct ParameterizedResult {
    /// Parameterized expression
    pub expression: Expression,
    /// List of extracted parameter values
    pub parameters: Vec<Value>,
    /// Parameter counter
    param_count: usize,
}

impl ParameterizedResult {
    fn new() -> Self {
        Self {
            expression: Expression::Literal(Value::Null(NullType::Null)),
            parameters: Vec::new(),
            param_count: 0,
        }
    }

    fn next_param_name(&mut self) -> String {
        self.param_count += 1;
        format!("${}", self.param_count)
    }

    fn add_parameter(&mut self, value: Value) -> String {
        let name = self.next_param_name();
        self.parameters.push(value);
        name
    }
}

/// Expression Parameterized Translator
///
/// Traverse the expression tree and replace all literals with parameter placeholders.
pub struct ParameterizingTransformer;

impl ParameterizingTransformer {
    pub fn new() -> Self {
        Self
    }

    /// Parameterizing a single ContextualExpression
    pub fn parameterize(&mut self, expr: &ContextualExpression) -> ParameterizedResult {
        let inner_expr = match expr.get_expression() {
            Some(e) => e,
            None => {
                return ParameterizedResult::new();
            }
        };
        let mut result = ParameterizedResult::new();
        let new_expr = self.transform_with_params(&inner_expr, &mut result);
        result.expression = new_expr;
        result
    }

    /// Parameterizing a single Expression (compatible with older interfaces)
    pub fn parameterize_expression(&mut self, expr: &Expression) -> ParameterizedResult {
        let mut result = ParameterizedResult::new();
        let new_expr = self.transform_with_params(expr, &mut result);
        result.expression = new_expr;
        result
    }

    /// Parameterizing multiple expressions
    pub fn parameterize_many(
        &mut self,
        exprs: &[ContextualExpression],
    ) -> Vec<ParameterizedResult> {
        exprs.iter().map(|expr| self.parameterize(expr)).collect()
    }

    fn transform_with_params(
        &mut self,
        expr: &Expression,
        result: &mut ParameterizedResult,
    ) -> Expression {
        match expr {
            Expression::Literal(value) => {
                // Replace the literals with parameter variables.
                let param_name = result.add_parameter(value.clone());
                Expression::Variable(param_name)
            }
            Expression::Binary { left, op, right } => {
                let new_left = self.transform_with_params(left, result);
                let new_right = self.transform_with_params(right, result);
                Expression::Binary {
                    left: Box::new(new_left),
                    op: *op,
                    right: Box::new(new_right),
                }
            }
            Expression::Unary { op, operand } => {
                let new_operand = self.transform_with_params(operand, result);
                Expression::Unary {
                    op: *op,
                    operand: Box::new(new_operand),
                }
            }
            Expression::Function { name, args } => {
                let new_args: Vec<Expression> = args
                    .iter()
                    .map(|arg| self.transform_with_params(arg, result))
                    .collect();
                Expression::Function {
                    name: name.clone(),
                    args: new_args,
                }
            }
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                let new_arg = self.transform_with_params(arg, result);
                Expression::Aggregate {
                    func: func.clone(),
                    arg: Box::new(new_arg),
                    distinct: *distinct,
                }
            }
            Expression::List(items) => {
                let new_items: Vec<Expression> = items
                    .iter()
                    .map(|item| self.transform_with_params(item, result))
                    .collect();
                Expression::List(new_items)
            }
            Expression::Map(pairs) => {
                let new_pairs: Vec<(String, Expression)> = pairs
                    .iter()
                    .map(|(k, v)| (k.clone(), self.transform_with_params(v, result)))
                    .collect();
                Expression::Map(new_pairs)
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let new_test_expr = test_expr
                    .as_ref()
                    .map(|e| Box::new(self.transform_with_params(e, result)));
                let new_conditions: Vec<(Expression, Expression)> = conditions
                    .iter()
                    .map(|(cond, val)| {
                        (
                            self.transform_with_params(cond, result),
                            self.transform_with_params(val, result),
                        )
                    })
                    .collect();
                let new_default = default
                    .as_ref()
                    .map(|d| Box::new(self.transform_with_params(d, result)));
                Expression::Case {
                    test_expr: new_test_expr,
                    conditions: new_conditions,
                    default: new_default,
                }
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let new_expr = self.transform_with_params(expression, result);
                Expression::TypeCast {
                    expression: Box::new(new_expr),
                    target_type: target_type.clone(),
                }
            }
            Expression::Subscript { collection, index } => {
                let new_collection = self.transform_with_params(collection, result);
                let new_index = self.transform_with_params(index, result);
                Expression::Subscript {
                    collection: Box::new(new_collection),
                    index: Box::new(new_index),
                }
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                let new_collection = self.transform_with_params(collection, result);
                let new_start = start
                    .as_ref()
                    .map(|s| Box::new(self.transform_with_params(s, result)));
                let new_end = end
                    .as_ref()
                    .map(|e| Box::new(self.transform_with_params(e, result)));
                Expression::Range {
                    collection: Box::new(new_collection),
                    start: new_start,
                    end: new_end,
                }
            }
            Expression::Path(items) => {
                let new_items: Vec<Expression> = items
                    .iter()
                    .map(|item| self.transform_with_params(item, result))
                    .collect();
                Expression::Path(new_items)
            }
            Expression::Property { object, property } => {
                let new_object = self.transform_with_params(object, result);
                Expression::Property {
                    object: Box::new(new_object),
                    property: property.clone(),
                }
            }
            Expression::ListComprehension {
                variable,
                source,
                filter,
                map,
            } => {
                let new_source = self.transform_with_params(source, result);
                let new_filter = filter
                    .as_ref()
                    .map(|f| Box::new(self.transform_with_params(f, result)));
                let new_map = map
                    .as_ref()
                    .map(|m| Box::new(self.transform_with_params(m, result)));
                Expression::ListComprehension {
                    variable: variable.clone(),
                    source: Box::new(new_source),
                    filter: new_filter,
                    map: new_map,
                }
            }
            Expression::LabelTagProperty { tag, property } => {
                let new_tag = self.transform_with_params(tag, result);
                Expression::LabelTagProperty {
                    tag: Box::new(new_tag),
                    property: property.clone(),
                }
            }
            Expression::Predicate { func, args } => {
                let new_args: Vec<Expression> = args
                    .iter()
                    .map(|arg| self.transform_with_params(arg, result))
                    .collect();
                Expression::Predicate {
                    func: func.clone(),
                    args: new_args,
                }
            }
            Expression::Reduce {
                accumulator,
                initial,
                variable,
                source,
                mapping,
            } => {
                let new_initial = self.transform_with_params(initial, result);
                let new_source = self.transform_with_params(source, result);
                let new_mapping = self.transform_with_params(mapping, result);
                Expression::Reduce {
                    accumulator: accumulator.clone(),
                    initial: Box::new(new_initial),
                    variable: variable.clone(),
                    source: Box::new(new_source),
                    mapping: Box::new(new_mapping),
                }
            }
            Expression::PathBuild(exprs) => {
                let new_exprs: Vec<Expression> = exprs
                    .iter()
                    .map(|e| self.transform_with_params(e, result))
                    .collect();
                Expression::PathBuild(new_exprs)
            }
            // The following types do not contain literals that require parameterization.
            Expression::Variable(_)
            | Expression::Label(_)
            | Expression::TagProperty { .. }
            | Expression::EdgeProperty { .. }
            | Expression::Parameter(_)
            | Expression::Vector(_) => expr.clone(),
        }
    }
}

impl Default for ParameterizingTransformer {
    fn default() -> Self {
        Self::new()
    }
}

/// Template extractor
///
/// Extract the parametric template from the sentence.
pub struct TemplateExtractor;

impl TemplateExtractor {
    /// Extract templates from the sentences.
    pub fn extract(stmt: &Stmt) -> String {
        match stmt {
            Stmt::Match(m) => Self::extract_match_template(m),
            Stmt::Go(g) => Self::extract_go_template(g),
            Stmt::Lookup(l) => Self::extract_lookup_template(l),
            Stmt::Fetch(f) => Self::extract_fetch_template(f),
            Stmt::Insert(i) => Self::extract_insert_template(i),
            Stmt::Delete(d) => Self::extract_delete_template(d),
            Stmt::Update(u) => Self::extract_update_template(u),
            _ => stmt.kind().to_string(),
        }
    }

    /// Extract the MATCH statement template.
    fn extract_match_template(stmt: &MatchStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        // Processing mode
        let pattern_template = Self::patterns_to_template(&stmt.patterns);
        parts.push(format!("MATCH {}", pattern_template));

        // Processing the WHERE clause
        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        // Handling the RETURN clause
        if let Some(ref return_clause) = stmt.return_clause {
            let return_template = Self::return_clause_to_template(return_clause, &mut transformer);
            parts.push(return_template);
        }

        // Handling the `ORDER BY` clause
        if let Some(ref order_by) = stmt.order_by {
            let order_items: Vec<String> = order_by
                .items
                .iter()
                .map(|item| {
                    let result = transformer.parameterize(&item.expression);
                    let expr_str = Self::expr_to_template_string(&result.expression);
                    let dir_str = match item.direction {
                        OrderDirection::Asc => "ASC",
                        OrderDirection::Desc => "DESC",
                    };
                    format!("{} {}", expr_str, dir_str)
                })
                .collect();
            parts.push(format!("ORDER BY {}", order_items.join(", ")));
        }

        // Handle the SKIP command.
        if let Some(skip) = stmt.skip {
            parts.push(format!("SKIP ${}", skip));
        }

        // Handling the LIMIT clause
        if let Some(limit) = stmt.limit {
            parts.push(format!("LIMIT ${}", limit));
        }

        // Handle the optional case.
        if stmt.optional {
            parts.insert(0, "OPTIONAL".to_string());
        }

        parts.join(" ")
    }

    /// Extract GO statement templates
    fn extract_go_template(stmt: &GoStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        // Number of steps
        let step_template = match &stmt.steps {
            crate::query::parser::ast::Steps::Fixed(n) => format!("{} STEPS", n),
            crate::query::parser::ast::Steps::Range { min, max } => {
                format!("{} TO {} STEPS", min, max)
            }
            crate::query::parser::ast::Steps::Variable(_) => "VARIABLE STEPS".to_string(),
        };
        parts.push(format!("GO {}", step_template));

        // FROM clause
        let from_template = Self::from_clause_to_template(&stmt.from, &mut transformer);
        parts.push(from_template);

        // OVER clause
        if let Some(ref over) = stmt.over {
            let edge_types = over.edge_types.join(", ");
            let dir_str = match over.direction {
                crate::query::parser::ast::EdgeDirection::Out => "",
                crate::query::parser::ast::EdgeDirection::In => "REVERSELY ",
                crate::query::parser::ast::EdgeDirection::Both => "BIDIRECT ",
            };
            parts.push(format!("OVER {}{}", dir_str, edge_types));
        }

        // The WHERE clause
        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        // YIELD clause
        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    /// Extract the LOOKUP statement template.
    fn extract_lookup_template(stmt: &LookupStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        // Of course! Please provide the text you would like to have translated.
        let target_str = match &stmt.target {
            crate::query::parser::ast::LookupTarget::Tag(name) => format!("ON {}", name),
            crate::query::parser::ast::LookupTarget::Edge(name) => format!("ON {}", name),
            crate::query::parser::ast::LookupTarget::Unspecified(name) => format!("ON {}", name),
        };
        parts.push(format!("LOOKUP {}", target_str));

        // The WHERE clause
        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        // YIELD clause
        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    /// Extract the FETCH statement template.
    fn extract_fetch_template(stmt: &FetchStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::query::parser::ast::FetchTarget::Vertices {
                ids, properties, ..
            } => {
                parts.push("FETCH VERTEX".to_string());

                // Parameterized vertex IDs
                let id_templates: Vec<String> = ids
                    .iter()
                    .map(|id| {
                        let result = transformer.parameterize(id);
                        Self::expr_to_template_string(&result.expression)
                    })
                    .collect();
                parts.push(id_templates.join(", "));

                // Attribute list
                if let Some(props) = properties {
                    parts.push(format!("YIELD {}", props.join(", ")));
                }
            }
            crate::query::parser::ast::FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                properties,
            } => {
                parts.push(format!("FETCH EDGE ON {}", edge_type));

                // Parameterized source and target
                let src_result = transformer.parameterize(src);
                let dst_result = transformer.parameterize(dst);
                parts.push(format!(
                    "{} -> {}",
                    Self::expr_to_template_string(&src_result.expression),
                    Self::expr_to_template_string(&dst_result.expression)
                ));

                // Sorting
                if let Some(r) = rank {
                    let rank_result = transformer.parameterize(r);
                    parts.push(format!(
                        "@{}",
                        Self::expr_to_template_string(&rank_result.expression)
                    ));
                }

                // Attribute list
                if let Some(props) = properties {
                    parts.push(format!("YIELD {}", props.join(", ")));
                }
            }
        }

        parts.join(" ")
    }

    /// Extract the INSERT statement template
    fn extract_insert_template(stmt: &InsertStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::query::parser::ast::InsertTarget::Vertices { tags, values } => {
                parts.push("INSERT VERTEX".to_string());

                // Tag list
                let tag_names: Vec<String> = tags.iter().map(|t| t.tag_name.clone()).collect();
                parts.push(tag_names.join(", "));

                // Attribute name
                for tag in tags {
                    if !tag.prop_names.is_empty() {
                        parts.push(format!("({})", tag.prop_names.join(", ")));
                    }
                }

                // VALUES
                parts.push("VALUES".to_string());

                // Parameterized values
                for row in values {
                    let vid_result = transformer.parameterize(&row.vid);
                    let vid_template = Self::expr_to_template_string(&vid_result.expression);

                    let tag_values_templates: Vec<String> = row
                        .tag_values
                        .iter()
                        .map(|values| {
                            let value_templates: Vec<String> = values
                                .iter()
                                .map(|v| {
                                    let result = transformer.parameterize(v);
                                    Self::expr_to_template_string(&result.expression)
                                })
                                .collect();
                            format!("({})", value_templates.join(", "))
                        })
                        .collect();

                    parts.push(format!(
                        "{}: {}",
                        vid_template,
                        tag_values_templates.join(", ")
                    ));
                }
            }
            crate::query::parser::ast::InsertTarget::Edge {
                edge_name,
                prop_names,
                edges,
            } => {
                parts.push(format!("INSERT EDGE {}", edge_name));

                // Attribute name
                if !prop_names.is_empty() {
                    parts.push(format!("({})", prop_names.join(", ")));
                }

                // VALUES
                parts.push("VALUES".to_string());

                // Parametric boundary values
                for (src, dst, rank, values) in edges {
                    let src_result = transformer.parameterize(src);
                    let dst_result = transformer.parameterize(dst);

                    let value_templates: Vec<String> = values
                        .iter()
                        .map(|v| {
                            let result = transformer.parameterize(v);
                            Self::expr_to_template_string(&result.expression)
                        })
                        .collect();

                    let mut edge_str = format!(
                        "{} -> {}",
                        Self::expr_to_template_string(&src_result.expression),
                        Self::expr_to_template_string(&dst_result.expression)
                    );

                    if let Some(r) = rank {
                        let rank_result = transformer.parameterize(r);
                        edge_str.push_str(&format!(
                            "@{}",
                            Self::expr_to_template_string(&rank_result.expression)
                        ));
                    }

                    if !value_templates.is_empty() {
                        edge_str.push_str(&format!(": ({})", value_templates.join(", ")));
                    }

                    parts.push(edge_str);
                }
            }
        }

        if stmt.if_not_exists {
            parts.push("IF NOT EXISTS".to_string());
        }

        parts.join(" ")
    }

    /// Extract the DELETE statement template.
    fn extract_delete_template(stmt: &DeleteStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::query::parser::ast::DeleteTarget::Vertices(exprs) => {
                parts.push("DELETE VERTEX".to_string());

                let id_templates: Vec<String> = exprs
                    .iter()
                    .map(|expr| {
                        let result = transformer.parameterize(expr);
                        Self::expr_to_template_string(&result.expression)
                    })
                    .collect();
                parts.push(id_templates.join(", "));
            }
            crate::query::parser::ast::DeleteTarget::Edges { edge_type, edges } => {
                if let Some(et) = edge_type {
                    parts.push(format!("DELETE EDGE {}", et));
                } else {
                    parts.push("DELETE EDGE".to_string());
                }

                for (src, dst, rank) in edges {
                    let src_result = transformer.parameterize(src);
                    let dst_result = transformer.parameterize(dst);

                    let mut edge_str = format!(
                        "{} -> {}",
                        Self::expr_to_template_string(&src_result.expression),
                        Self::expr_to_template_string(&dst_result.expression)
                    );

                    if let Some(r) = rank {
                        let rank_result = transformer.parameterize(r);
                        edge_str.push_str(&format!(
                            "@{}",
                            Self::expr_to_template_string(&rank_result.expression)
                        ));
                    }

                    parts.push(edge_str);
                }
            }
            crate::query::parser::ast::DeleteTarget::Tags {
                tag_names,
                vertex_ids,
                is_all_tags,
            } => {
                if *is_all_tags {
                    parts.push("DELETE TAG *".to_string());
                } else {
                    parts.push(format!("DELETE TAG {}", tag_names.join(", ")));
                }

                let id_templates: Vec<String> = vertex_ids
                    .iter()
                    .map(|expr| {
                        let result = transformer.parameterize(expr);
                        Self::expr_to_template_string(&result.expression)
                    })
                    .collect();
                parts.push(id_templates.join(", "));
            }
            crate::query::parser::ast::DeleteTarget::Index(name) => {
                parts.push(format!("DELETE INDEX {}", name));
            }
        }

        // The WHERE clause
        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if stmt.with_edge {
            parts.push("WITH EDGE".to_string());
        }

        parts.join(" ")
    }

    /// Extract the UPDATE statement template.
    fn extract_update_template(stmt: &UpdateStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        parts.push("UPDATE".to_string());

        // Of course! Please provide the text you would like to have translated.
        match &stmt.target {
            crate::query::parser::ast::UpdateTarget::Vertex(expr) => {
                let result = transformer.parameterize(expr);
                parts.push(format!(
                    "VERTEX {}",
                    Self::expr_to_template_string(&result.expression)
                ));
            }
            crate::query::parser::ast::UpdateTarget::Edge {
                src,
                dst,
                edge_type,
                rank,
            } => {
                let src_result = transformer.parameterize(src);
                let dst_result = transformer.parameterize(dst);

                let mut edge_str = format!(
                    "EDGE {} -> {}",
                    Self::expr_to_template_string(&src_result.expression),
                    Self::expr_to_template_string(&dst_result.expression)
                );

                if let Some(et) = edge_type {
                    edge_str.push_str(&format!(" OF {}", et));
                }

                if let Some(r) = rank {
                    let rank_result = transformer.parameterize(r);
                    edge_str.push_str(&format!(
                        "@{}",
                        Self::expr_to_template_string(&rank_result.expression)
                    ));
                }

                parts.push(edge_str);
            }
            crate::query::parser::ast::UpdateTarget::Tag(name) => {
                parts.push(format!("TAG {}", name));
            }
            crate::query::parser::ast::UpdateTarget::TagOnVertex { vid, tag_name } => {
                let vid_result = transformer.parameterize(vid);
                parts.push(format!(
                    "VERTEX {} ON {}",
                    Self::expr_to_template_string(&vid_result.expression),
                    tag_name
                ));
            }
        }

        // SET clause
        let set_template = Self::set_clause_to_template(&stmt.set_clause, &mut transformer);
        parts.push(set_template);

        // The WHERE clause
        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if stmt.is_upsert {
            parts.push("UPSERT".to_string());
        }

        // The YIELD clause
        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    // Auxiliary methods

    /// Convert the list of patterns into template strings.
    fn patterns_to_template(patterns: &[Pattern]) -> String {
        patterns
            .iter()
            .map(Self::pattern_to_template)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Convert a single pattern into a template string.
    fn pattern_to_template(pattern: &Pattern) -> String {
        match pattern {
            Pattern::Node(node) => {
                let mut parts = Vec::new();

                if let Some(ref var) = node.variable {
                    parts.push(var.clone());
                }

                if !node.labels.is_empty() {
                    parts.push(format!(":{}", node.labels.join(":")));
                }

                if node.properties.is_some() {
                    parts.push("{...}".to_string());
                }

                if !node.predicates.is_empty() {
                    parts.push("WHERE ...".to_string());
                }

                format!("({})", parts.join(""))
            }
            Pattern::Edge(edge) => {
                let mut parts = Vec::new();

                if let Some(ref var) = edge.variable {
                    parts.push(var.clone());
                }

                if !edge.edge_types.is_empty() {
                    parts.push(format!(":{}", edge.edge_types.join("|")));
                }

                if edge.properties.is_some() {
                    parts.push("{...}".to_string());
                }

                let (prefix, suffix) = match edge.direction {
                    crate::query::parser::ast::EdgeDirection::Out => ("-[", "]->"),
                    crate::query::parser::ast::EdgeDirection::In => ("<-[", "]-"),
                    crate::query::parser::ast::EdgeDirection::Both => ("-[", "]-"),
                };

                format!("{}{}{}", prefix, parts.join(""), suffix)
            }
            Pattern::Path(path) => {
                let elements: Vec<String> = path
                    .elements
                    .iter()
                    .map(|e| match e {
                        crate::query::parser::ast::PathElement::Node(n) => {
                            Self::pattern_to_template(&Pattern::Node(n.clone()))
                        }
                        crate::query::parser::ast::PathElement::Edge(e) => {
                            Self::pattern_to_template(&Pattern::Edge(e.clone()))
                        }
                        crate::query::parser::ast::PathElement::Alternative(patterns) => {
                            let alts: Vec<String> =
                                patterns.iter().map(Self::pattern_to_template).collect();
                            format!("({})", alts.join(" | "))
                        }
                        crate::query::parser::ast::PathElement::Optional(elem) => {
                            format!("{}?", Self::path_element_to_template(elem))
                        }
                        crate::query::parser::ast::PathElement::Repeated(elem, rep) => {
                            let rep_str = match rep {
                                crate::query::parser::ast::RepetitionType::ZeroOrMore => "*",
                                crate::query::parser::ast::RepetitionType::OneOrMore => "+",
                                crate::query::parser::ast::RepetitionType::ZeroOrOne => "?",
                                crate::query::parser::ast::RepetitionType::Exactly(n) => {
                                    &format!("{{{}}}", n)
                                }
                                crate::query::parser::ast::RepetitionType::Range(min, max) => {
                                    &format!("{{{},{}}}", min, max)
                                }
                            };
                            format!("{}{}", Self::path_element_to_template(elem), rep_str)
                        }
                    })
                    .collect();
                elements.join("")
            }
            Pattern::Variable(var) => format!("@{}", var.name),
        }
    }

    /// Convert the path elements into template strings.
    fn path_element_to_template(elem: &crate::query::parser::ast::PathElement) -> String {
        match elem {
            crate::query::parser::ast::PathElement::Node(n) => {
                Self::pattern_to_template(&Pattern::Node(n.clone()))
            }
            crate::query::parser::ast::PathElement::Edge(e) => {
                Self::pattern_to_template(&Pattern::Edge(e.clone()))
            }
            _ => "(...)".to_string(),
        }
    }

    /// Translate the following text into a template string:
    fn expr_to_template_string(expr: &Expression) -> String {
        match expr {
            Expression::Variable(name) if name.starts_with('$') => name.clone(),
            Expression::Variable(name) => name.clone(),
            Expression::Literal(value) => format!("{:?}", value),
            Expression::Property { object, property } => {
                format!("{}.{}", Self::expr_to_template_string(object), property)
            }
            Expression::Binary { left, op, right } => {
                format!(
                    "({} {} {})",
                    Self::expr_to_template_string(left),
                    op,
                    Self::expr_to_template_string(right)
                )
            }
            Expression::Unary { op, operand } => {
                format!("({}{})", op, Self::expr_to_template_string(operand))
            }
            Expression::Function { name, args } => {
                let arg_strs: Vec<String> =
                    args.iter().map(Self::expr_to_template_string).collect();
                format!("{}({})", name, arg_strs.join(", "))
            }
            Expression::Aggregate {
                func,
                arg,
                distinct,
            } => {
                let distinct_str = if *distinct { "DISTINCT " } else { "" };
                format!(
                    "{}({}{})",
                    func,
                    distinct_str,
                    Self::expr_to_template_string(arg)
                )
            }
            Expression::List(items) => {
                let item_strs: Vec<String> =
                    items.iter().map(Self::expr_to_template_string).collect();
                format!("[{}]", item_strs.join(", "))
            }
            Expression::Map(pairs) => {
                let pair_strs: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, Self::expr_to_template_string(v)))
                    .collect();
                format!("{{{}}}", pair_strs.join(", "))
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                let mut parts = Vec::new();
                parts.push("CASE".to_string());

                if let Some(test) = test_expr {
                    parts.push(Self::expr_to_template_string(test));
                }

                for (cond, val) in conditions {
                    parts.push(format!(
                        "WHEN {} THEN {}",
                        Self::expr_to_template_string(cond),
                        Self::expr_to_template_string(val)
                    ));
                }

                if let Some(def) = default {
                    parts.push(format!("ELSE {}", Self::expr_to_template_string(def)));
                }

                parts.push("END".to_string());
                parts.join(" ")
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                format!(
                    "CAST({} AS {:?})",
                    Self::expr_to_template_string(expression),
                    target_type
                )
            }
            Expression::Subscript { collection, index } => {
                format!(
                    "{}[{}]",
                    Self::expr_to_template_string(collection),
                    Self::expr_to_template_string(index)
                )
            }
            Expression::Label(name) => name.clone(),
            Expression::Parameter(name) => format!("${}", name),
            _ => "...".to_string(),
        }
    }

    /// Convert the RETURN statement into a template string.
    fn return_clause_to_template(
        clause: &ReturnClause,
        transformer: &mut ParameterizingTransformer,
    ) -> String {
        let mut parts = Vec::new();

        if clause.distinct {
            parts.push("DISTINCT".to_string());
        }

        let item_strs: Vec<String> = clause
            .items
            .iter()
            .map(|item| match item {
                ReturnItem::Expression { expression, alias } => {
                    let result = transformer.parameterize(expression);
                    let mut expr_str = Self::expr_to_template_string(&result.expression);
                    if let Some(a) = alias {
                        expr_str.push_str(&format!(" AS {}", a));
                    }
                    expr_str
                }
            })
            .collect();

        parts.push(item_strs.join(", "));

        format!("RETURN {}", parts.join(" "))
    }

    /// Convert the YIELD clause into a template string.
    fn yield_clause_to_template(
        clause: &YieldClause,
        transformer: &mut ParameterizingTransformer,
    ) -> String {
        let mut parts = Vec::new();

        let item_strs: Vec<String> = clause
            .items
            .iter()
            .map(|item| {
                let result = transformer.parameterize(&item.expression);
                let mut expr_str = Self::expr_to_template_string(&result.expression);
                if let Some(ref a) = item.alias {
                    expr_str.push_str(&format!(" AS {}", a));
                }
                expr_str
            })
            .collect();

        parts.push(format!("YIELD {}", item_strs.join(", ")));

        // WHERE
        if let Some(ref where_expr) = clause.where_clause {
            let result = transformer.parameterize(where_expr);
            parts.push(format!(
                "WHERE {}",
                Self::expr_to_template_string(&result.expression)
            ));
        }

        parts.join(" ").to_string()
    }

    /// Convert the FROM clause into a template string.
    fn from_clause_to_template(
        clause: &FromClause,
        transformer: &mut ParameterizingTransformer,
    ) -> String {
        let vertex_strs: Vec<String> = clause
            .vertices
            .iter()
            .map(|expr| {
                let result = transformer.parameterize(expr);
                Self::expr_to_template_string(&result.expression)
            })
            .collect();

        format!("FROM {}", vertex_strs.join(", "))
    }

    /// Convert the SET statement into a template string.
    fn set_clause_to_template(
        clause: &SetClause,
        transformer: &mut ParameterizingTransformer,
    ) -> String {
        let assignment_strs: Vec<String> = clause
            .assignments
            .iter()
            .map(|assign| {
                let result = transformer.parameterize(&assign.value);
                format!(
                    "{} = {}",
                    assign.property,
                    Self::expr_to_template_string(&result.expression)
                )
            })
            .collect();

        format!("SET {}", assignment_strs.join(", "))
    }
}

impl Default for TemplateExtractor {
    fn default() -> Self {
        Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    #[test]
    fn test_parameterize_literal() {
        let mut transformer = ParameterizingTransformer::new();
        let expr = Expression::Literal(Value::Int(42));
        let result = transformer.parameterize_expression(&expr);

        assert_eq!(result.parameters.len(), 1);
        assert_eq!(result.parameters[0], Value::Int(42));
        assert!(matches!(result.expression, Expression::Variable(ref name) if name == "$1"));
    }

    #[test]
    fn test_parameterize_binary_expr() {
        use crate::core::types::operators::BinaryOperator;

        let mut transformer = ParameterizingTransformer::new();
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("age".to_string())),
            op: BinaryOperator::GreaterThan,
            right: Box::new(Expression::Literal(Value::Int(18))),
        };
        let result = transformer.parameterize_expression(&expr);

        assert_eq!(result.parameters.len(), 1);
        assert_eq!(result.parameters[0], Value::Int(18));
    }

    #[test]
    fn test_expr_to_template_string() {
        let expr = Expression::Binary {
            left: Box::new(Expression::Variable("$1".to_string())),
            op: crate::core::types::operators::BinaryOperator::Equal,
            right: Box::new(Expression::Variable("name".to_string())),
        };

        let template = TemplateExtractor::expr_to_template_string(&expr);
        assert!(template.contains("$1"));
        assert!(template.contains("name"));
    }
}

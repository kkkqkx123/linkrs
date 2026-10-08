use super::ParameterizingTransformer;
use crate::parser::ast::stmt::OrderDirection;
use crate::parser::ast::stmt::{
    DeleteStmt, FetchStmt, FromClause, GoStmt, InsertStmt, LookupStmt, MatchStmt, Pattern,
    ReturnClause, ReturnItem, SetClause, Stmt, UpdateStmt, YieldClause,
};
use linkrs_core::types::expr::Expression;

#[derive(Debug)]
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
            _ => stmt.category().as_str().to_string(),
        }
    }

    fn extract_match_template(stmt: &MatchStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        let pattern_template = Self::patterns_to_template(&stmt.patterns);
        parts.push(format!("MATCH {}", pattern_template));

        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if let Some(ref return_clause) = stmt.return_clause {
            let return_template = Self::return_clause_to_template(return_clause, &mut transformer);
            parts.push(return_template);
        }

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

        if let Some(skip) = &stmt.skip {
            parts.push(format!("SKIP ${}", skip.count));
        }

        if let Some(limit) = &stmt.limit {
            parts.push(format!("LIMIT ${}", limit.count));
        }

        if stmt.optional {
            parts.insert(0, "OPTIONAL".to_string());
        }

        parts.join(" ")
    }

    fn extract_go_template(stmt: &GoStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        let step_template = match &stmt.steps {
            crate::parser::ast::Steps::Fixed(n) => format!("{} STEPS", n),
            crate::parser::ast::Steps::Range { min, max } => {
                format!("{} TO {} STEPS", min, max)
            }
            crate::parser::ast::Steps::Variable(_) => "VARIABLE STEPS".to_string(),
        };
        parts.push(format!("GO {}", step_template));

        let from_template = Self::from_clause_to_template(&stmt.from, &mut transformer);
        parts.push(from_template);

        if let Some(ref over) = stmt.over {
            let edge_types = over.edge_types.join(", ");
            let dir_str = match over.direction {
                crate::parser::ast::EdgeDirection::Out => "",
                crate::parser::ast::EdgeDirection::In => "REVERSELY ",
                crate::parser::ast::EdgeDirection::Both => "BIDIRECT ",
            };
            parts.push(format!("OVER {}{}", dir_str, edge_types));
        }

        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    fn extract_lookup_template(stmt: &LookupStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        let target_str = match &stmt.target {
            crate::parser::ast::LookupTarget::Tag(name) => format!("ON {}", name),
            crate::parser::ast::LookupTarget::Edge(name) => format!("ON {}", name),
            crate::parser::ast::LookupTarget::Unspecified(name) => format!("ON {}", name),
        };
        parts.push(format!("LOOKUP {}", target_str));

        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    fn extract_fetch_template(stmt: &FetchStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::parser::ast::FetchTarget::Vertices {
                ids, properties, ..
            } => {
                parts.push("FETCH VERTEX".to_string());

                let id_templates: Vec<String> = ids
                    .iter()
                    .map(|id| {
                        let result = transformer.parameterize(id);
                        Self::expr_to_template_string(&result.expression)
                    })
                    .collect();
                parts.push(id_templates.join(", "));

                if let Some(props) = properties {
                    parts.push(format!("YIELD {}", props.join(", ")));
                }
            }
            crate::parser::ast::FetchTarget::Edges {
                src,
                dst,
                edge_type,
                rank,
                properties,
            } => {
                parts.push(format!("FETCH EDGE ON {}", edge_type));

                let src_result = transformer.parameterize(src);
                let dst_result = transformer.parameterize(dst);
                parts.push(format!(
                    "{} -> {}",
                    Self::expr_to_template_string(&src_result.expression),
                    Self::expr_to_template_string(&dst_result.expression)
                ));

                if let Some(r) = rank {
                    let rank_result = transformer.parameterize(r);
                    parts.push(format!(
                        "@{}",
                        Self::expr_to_template_string(&rank_result.expression)
                    ));
                }

                if let Some(props) = properties {
                    parts.push(format!("YIELD {}", props.join(", ")));
                }
            }
        }

        parts.join(" ")
    }

    fn extract_insert_template(stmt: &InsertStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::parser::ast::InsertTarget::Vertices { tag, values } => {
                parts.push("INSERT VERTEX".to_string());

                parts.push(tag.tag_name.clone());

                if !tag.prop_names.is_empty() {
                    parts.push(format!("({})", tag.prop_names.join(", ")));
                }

                parts.push("VALUES".to_string());

                for row in values {
                    let vid_result = transformer.parameterize(&row.vid);
                    let vid_template = Self::expr_to_template_string(&vid_result.expression);

                    let value_templates: Vec<String> = row
                        .values
                        .iter()
                        .map(|v| {
                            let result = transformer.parameterize(v);
                            Self::expr_to_template_string(&result.expression)
                        })
                        .collect();
                    let tag_values_templates = [format!("({})", value_templates.join(", "))];

                    parts.push(format!(
                        "{}: {}",
                        vid_template,
                        tag_values_templates.join(", ")
                    ));
                }
            }
            crate::parser::ast::InsertTarget::Edge {
                edge_name,
                prop_names,
                edges,
            } => {
                parts.push(format!("INSERT EDGE {}", edge_name));

                if !prop_names.is_empty() {
                    parts.push(format!("({})", prop_names.join(", ")));
                }

                parts.push("VALUES".to_string());

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

    fn extract_delete_template(stmt: &DeleteStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        match &stmt.target {
            crate::parser::ast::DeleteTarget::Vertices { tag, vids } => {
                parts.push(format!("DELETE VERTEX {} FROM", tag));

                let id_templates: Vec<String> = vids
                    .iter()
                    .map(|expr| {
                        let result = transformer.parameterize(expr);
                        Self::expr_to_template_string(&result.expression)
                    })
                    .collect();
                parts.push(id_templates.join(", "));
            }
            crate::parser::ast::DeleteTarget::Edges { edge_type, edges } => {
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
            crate::parser::ast::DeleteTarget::Index(name) => {
                parts.push(format!("DELETE INDEX {}", name));
            }
        }

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

    fn extract_update_template(stmt: &UpdateStmt) -> String {
        let mut transformer = ParameterizingTransformer::new();
        let mut parts = Vec::new();

        parts.push("UPDATE".to_string());

        match &stmt.target {
            crate::parser::ast::UpdateTarget::Vertex(expr) => {
                let result = transformer.parameterize(expr);
                parts.push(format!(
                    "VERTEX {}",
                    Self::expr_to_template_string(&result.expression)
                ));
            }
            crate::parser::ast::UpdateTarget::Edge {
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
            crate::parser::ast::UpdateTarget::Tag(name) => {
                parts.push(format!("TAG {}", name));
            }
            crate::parser::ast::UpdateTarget::TagOnVertex { vid, tag_name } => {
                let vid_result = transformer.parameterize(vid);
                parts.push(format!(
                    "VERTEX {} ON {}",
                    Self::expr_to_template_string(&vid_result.expression),
                    tag_name
                ));
            }
        }

        let set_template = Self::set_clause_to_template(&stmt.set_clause, &mut transformer);
        parts.push(set_template);

        if let Some(ref where_expr) = stmt.where_clause {
            let result = transformer.parameterize(where_expr);
            let where_template = Self::expr_to_template_string(&result.expression);
            parts.push(format!("WHERE {}", where_template));
        }

        if stmt.is_upsert {
            parts.push("UPSERT".to_string());
        }

        if let Some(ref yield_clause) = stmt.yield_clause {
            let yield_template = Self::yield_clause_to_template(yield_clause, &mut transformer);
            parts.push(yield_template);
        }

        parts.join(" ")
    }

    fn patterns_to_template(patterns: &[Pattern]) -> String {
        patterns
            .iter()
            .map(Self::pattern_to_template)
            .collect::<Vec<_>>()
            .join(", ")
    }

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
                    crate::parser::ast::EdgeDirection::Out => ("-[", "]->"),
                    crate::parser::ast::EdgeDirection::In => ("<-[", "]-"),
                    crate::parser::ast::EdgeDirection::Both => ("-[", "]-"),
                };

                format!("{}{}{}", prefix, parts.join(""), suffix)
            }
            Pattern::Path(path) => {
                let elements: Vec<String> = path
                    .elements
                    .iter()
                    .map(|e| match e {
                        crate::parser::ast::PathElement::Node(n) => {
                            Self::pattern_to_template(&Pattern::Node(n.clone()))
                        }
                        crate::parser::ast::PathElement::Edge(e) => {
                            Self::pattern_to_template(&Pattern::Edge(e.clone()))
                        }
                        crate::parser::ast::PathElement::Alternative(patterns) => {
                            let alts: Vec<String> =
                                patterns.iter().map(Self::pattern_to_template).collect();
                            format!("({})", alts.join(" | "))
                        }
                        crate::parser::ast::PathElement::Optional(elem) => {
                            format!("{}?", Self::path_element_to_template(elem))
                        }
                        crate::parser::ast::PathElement::Repeated(elem, rep) => {
                            let rep_str = match rep {
                                crate::parser::ast::RepetitionType::ZeroOrMore => "*",
                                crate::parser::ast::RepetitionType::OneOrMore => "+",
                                crate::parser::ast::RepetitionType::ZeroOrOne => "?",
                                crate::parser::ast::RepetitionType::Exactly(n) => {
                                    &format!("{{{}}}", n)
                                }
                                crate::parser::ast::RepetitionType::Range(min, max) => {
                                    &format!("{{{},{}}}", min, max)
                                }
                            };
                            format!("{}{}", Self::path_element_to_template(elem), rep_str)
                        }
                        crate::parser::ast::PathElement::Recursive(rc) => {
                            let mut parts = vec![rc.variable.clone()];
                            if let Some(ref edge_var) = rc.edge_variable {
                                parts.push(edge_var.clone());
                            }
                            if rc.filter_predicate.is_some() {
                                parts.push("WHERE".to_string());
                            }
                            if rc.node_projection.is_some() || rc.edge_projection.is_some() {
                                parts.push("|".to_string());
                                let mut projs = vec![];
                                if rc.node_projection.is_some() {
                                    projs.push("{node}".to_string());
                                }
                                if rc.edge_projection.is_some() {
                                    projs.push("{edge}".to_string());
                                }
                                parts.push(projs.join(", "));
                            }
                            format!("*({})", parts.join(", "))
                        }
                    })
                    .collect();
                elements.join("")
            }
            Pattern::Variable(var) => format!("@{}", var.name),
        }
    }

    fn path_element_to_template(elem: &crate::parser::ast::PathElement) -> String {
        match elem {
            crate::parser::ast::PathElement::Node(n) => {
                Self::pattern_to_template(&Pattern::Node(n.clone()))
            }
            crate::parser::ast::PathElement::Edge(e) => {
                Self::pattern_to_template(&Pattern::Edge(e.clone()))
            }
            _ => "(...)".to_string(),
        }
    }

    pub(crate) fn expr_to_template_string(expr: &Expression) -> String {
        match expr {
            Expression::Variable(name) if name.starts_with('$') => name.clone(),
            Expression::Variable(name) => name.clone(),
            Expression::Literal(value) => format!("{:?}", value),
            Expression::Property { object, property } => {
                format!("{}.{}", Self::expr_to_template_string(object), property)
            }
            Expression::StructField { base, field } => {
                format!("{}.{}", Self::expr_to_template_string(base), field)
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
                let arg_strs: Vec<String> = args
                    .iter()
                    .map(|a| Self::expr_to_template_string(a.as_expr()))
                    .collect();
                format!("{}({})", name, arg_strs.join(", "))
            }
            Expression::Aggregate {
                func,
                args,
                distinct,
                filter,
            } => {
                let distinct_str = if *distinct { "DISTINCT " } else { "" };
                let filter_str = filter
                    .as_ref()
                    .map(|f| format!(" FILTER (WHERE {})", Self::expr_to_template_string(f)))
                    .unwrap_or_default();
                let arg_strs: Vec<String> =
                    args.iter().map(Self::expr_to_template_string).collect();
                format!(
                    "{}({}{}{})",
                    func,
                    distinct_str,
                    arg_strs.join(", "),
                    filter_str
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
            Expression::Parameter(name) => format!("@{}", name),
            Expression::SessionVariable(name) => format!("${}", name),
            _ => "...".to_string(),
        }
    }

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

        if let Some(ref where_expr) = clause.where_clause {
            let result = transformer.parameterize(where_expr);
            parts.push(format!(
                "WHERE {}",
                Self::expr_to_template_string(&result.expression)
            ));
        }

        parts.join(" ").to_string()
    }

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
        TemplateExtractor
    }
}

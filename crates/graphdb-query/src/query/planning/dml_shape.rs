//! DML shape normalization for plan-cache reuse.
//!
//! Same-shape DML statements (e.g. 10000 identical INSERT VERTEX templates
//! that only differ in literal values) should compile their physical plan
//! once and reuse it from the plan cache, binding per-statement literal
//! values at execution time.
//!
//! This module re-renders a DML statement as a canonical template in which
//! every literal value position is replaced by a `$__dml_N` parameter slot,
//! producing a normalized query text (identical across same-shape
//! statements) together with the ordered literal values used for
//! execution-time binding.
//!
//! Scope: the direct write statements (INSERT / DELETE / UPDATE / MERGE /
//! SET / REMOVE) — see `crate::query::pipeline::prepared::is_direct_dml`
//! for the authoritative set.
//! Read queries are deliberately excluded: their plans depend on constant
//! values (constant folding, index selection) and users already have
//! explicit `$param` binding for parameterized read reuse.
//!
//! Semantic note: after normalization the bound DML plan sees only `$__dml_N`
//! parameters, not the literal values. DML plans do not rely on constant
//! values for constant folding or index selection, so parameterization does
//! not degrade plan quality — the same rationale that excludes read queries.

use crate::core::types::expr::{ContextualExpression, Expression};
use crate::core::Value;
use crate::query::parser::ast::Stmt;

mod delete;
mod insert;
mod merge;
mod set_remove;
mod update;

/// Prefix used for synthesized DML parameter names.
pub const DML_PARAM_PREFIX: &str = "__dml_";

/// Result of normalizing a DML statement's shape.
#[derive(Debug, Clone)]
pub struct DmlShape {
    /// Query text with literal values replaced by `$__dml_N` placeholders.
    pub normalized_text: String,
    /// Literal values in left-to-right source order, indexed by `N`.
    pub values: Vec<Value>,
}

/// Normalize a DML statement, returning the canonical template text and the
/// ordered literal values.
///
/// Returns `None` when the statement is not shape-cacheable (e.g. a value
/// position uses a construct that cannot be re-rendered as a parameter).
pub fn normalize_shape(stmt: &Stmt) -> Option<DmlShape> {
    let mut values = Vec::new();
    let normalized_text = match stmt {
        Stmt::Insert(s) => insert::render_insert(s, &mut values)?,
        Stmt::Delete(s) => delete::render_delete(s, &mut values)?,
        Stmt::Update(s) => update::render_update(s, &mut values)?,
        Stmt::Merge(s) => merge::render_merge(s, &mut values)?,
        Stmt::Set(s) => set_remove::render_set(s, &mut values)?,
        Stmt::Remove(s) => set_remove::render_remove(s, &mut values)?,
        _ => return None,
    };
    if values.is_empty() {
        return None;
    }
    Some(DmlShape {
        normalized_text,
        values,
    })
}

/// Render an expression position, replacing every literal leaf with a
/// `$__dml_N` placeholder and recording the value.
///
/// Returns `None` when the expression contains a construct that cannot be
/// re-rendered as re-parseable text; the caller then treats the statement as
/// not shape-cacheable instead of risking a broken template.
pub(crate) fn render_contextual(
    out: &mut String,
    values: &mut Vec<Value>,
    expr: &ContextualExpression,
) -> Option<()> {
    render_expr(out, values, expr.expression()?.inner())
}

/// Render an expression into `out`, replacing every literal leaf with a
/// `$__dml_N` placeholder.
///
/// Only constructs that can be faithfully re-parsed are handled; anything
/// else yields `None` so the statement falls back to the non-cached path.
fn render_expr(out: &mut String, values: &mut Vec<Value>, expr: &Expression) -> Option<()> {
    match expr {
        Expression::Literal(value) => {
            push_param(out, values, value.clone());
            Some(())
        }
        Expression::Variable(name) => {
            out.push_str(name);
            Some(())
        }
        Expression::Parameter(name) => {
            out.push('$');
            out.push_str(name);
            Some(())
        }
        Expression::Property { object, property } => {
            render_expr(out, values, object)?;
            out.push('.');
            out.push_str(property);
            Some(())
        }
        Expression::Binary { left, op, right } => {
            // `IN`/`NOT IN` and the subscript/attribute/JSONB operators cannot
            // be re-rendered as `(left OP right)` text; exclude them so the
            // statement falls back instead of producing a broken template.
            if matches!(
                op,
                crate::core::types::operators::BinaryOperator::In
                    | crate::core::types::operators::BinaryOperator::NotIn
                    | crate::core::types::operators::BinaryOperator::Subscript
                    | crate::core::types::operators::BinaryOperator::Attribute
                    | crate::core::types::operators::BinaryOperator::JsonGet
                    | crate::core::types::operators::BinaryOperator::JsonGetText
                    | crate::core::types::operators::BinaryOperator::JsonPathGet
                    | crate::core::types::operators::BinaryOperator::JsonPathGetText
            ) {
                return None;
            }
            out.push('(');
            render_expr(out, values, left)?;
            out.push(' ');
            out.push_str(op.name());
            out.push(' ');
            render_expr(out, values, right)?;
            out.push(')');
            Some(())
        }
        Expression::Unary { op, operand } => {
            out.push('(');
            out.push_str(op.name());
            out.push(' ');
            render_expr(out, values, operand)?;
            out.push(')');
            Some(())
        }
        Expression::Function { name, args } => {
            out.push_str(name);
            out.push('(');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                render_expr(out, values, arg)?;
            }
            out.push(')');
            Some(())
        }
        Expression::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                render_expr(out, values, item)?;
            }
            out.push(']');
            Some(())
        }
        Expression::Map(pairs) => {
            out.push('{');
            for (index, (key, value)) in pairs.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(key);
                out.push_str(": ");
                render_expr(out, values, value)?;
            }
            out.push('}');
            Some(())
        }
        Expression::Label(name) => {
            out.push(':');
            out.push_str(name);
            Some(())
        }
        Expression::TagProperty { tag_name, property } => {
            out.push_str(tag_name);
            out.push('.');
            out.push_str(property);
            Some(())
        }
        Expression::EdgeProperty {
            edge_name,
            property,
        } => {
            out.push_str(edge_name);
            out.push('.');
            out.push_str(property);
            Some(())
        }
        Expression::LabelTagProperty { tag, property } => {
            match tag.as_ref() {
                Expression::Variable(name) => out.push_str(name),
                Expression::Label(name) => out.push_str(name),
                _ => return None,
            }
            out.push('.');
            out.push_str(property);
            Some(())
        }
        _ => None,
    }
}

/// Emit a `$__dml_N` placeholder and record the literal value.
fn push_param(out: &mut String, values: &mut Vec<Value>, value: Value) {
    let index = values.len();
    out.push('$');
    out.push_str(DML_PARAM_PREFIX);
    out.push_str(&index.to_string());
    values.push(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parser::ast::{InsertTarget, UpdateTarget};
    use crate::query::parser::Parser;

    fn parse(query: &str) -> Stmt {
        let mut parser = Parser::new(query);
        let result = parser
            .parse()
            .unwrap_or_else(|e| panic!("parse failed for {query:?}: {e}"));
        assert!(!parser.has_errors(), "parse errors for {query:?}");
        result.ast.stmt().clone()
    }

    #[test]
    fn normalizes_insert_vertex_shape() {
        let query = "INSERT VERTEX person(name, age) VALUES \"p00001\": (\"Alice\", 30)";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "INSERT VERTEX person(name, age) VALUES $__dml_0: ($__dml_1, $__dml_2)"
        );
        assert_eq!(shape.values.len(), 3);
        assert_eq!(shape.values[0], Value::from("p00001"));
        assert_eq!(shape.values[1], Value::from("Alice"));
        assert_eq!(shape.values[2], Value::from(30i64));
    }

    #[test]
    fn normalizes_insert_edge_shape() {
        let query = "INSERT EDGE works_at(position, salary) VALUES \"p00001\" -> \"c0001\" @0: (\"Engineer\", 45000)";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "INSERT EDGE works_at(position, salary) VALUES $__dml_0 -> $__dml_1 @$__dml_2: ($__dml_3, $__dml_4)"
        );
        assert_eq!(shape.values.len(), 5);
    }

    #[test]
    fn same_shape_produces_same_normalized_text() {
        let a = parse("INSERT VERTEX user(id, name) VALUES \"u1\": (\"Bob\", 1)");
        let b = parse("INSERT VERTEX user(id, name) VALUES \"u2\": (\"Alice\", 2)");
        let shape_a = normalize_shape(&a).expect("shape a");
        let shape_b = normalize_shape(&b).expect("shape b");
        assert_eq!(shape_a.normalized_text, shape_b.normalized_text);
    }

    #[test]
    fn normalized_text_reparses_with_parameters() {
        let query = "INSERT VERTEX person(name, age) VALUES \"p00001\": (\"Alice\", 30)";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        let reparsed = parse(&shape.normalized_text);
        match reparsed {
            Stmt::Insert(insert) => {
                assert!(matches!(insert.target, InsertTarget::Vertices { .. }));
                if let InsertTarget::Vertices { values, .. } = &insert.target {
                    assert!(values[0].vid.is_parameter());
                }
            }
            _ => panic!("reparse should produce INSERT"),
        }
    }

    #[test]
    fn normalizes_if_not_exists_and_multi_row() {
        let query =
            "INSERT VERTEX IF NOT EXISTS person(name) VALUES \"p1\": (\"A\"), \"p2\": (\"B\")";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "INSERT VERTEX IF NOT EXISTS person(name) VALUES $__dml_0: ($__dml_1), $__dml_2: ($__dml_3)"
        );
        assert_eq!(shape.values.len(), 4);
    }

    #[test]
    fn parameterizes_nested_literal_in_insert() {
        let query = "INSERT VERTEX user(id, name) VALUES \"u1\": (upper(\"bob\"), 1)";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "INSERT VERTEX User(id, name) VALUES $__dml_0: (upper($__dml_1), $__dml_2)"
        );
    }

    #[test]
    fn skips_unrenderable_expression() {
        let query = "UPDATE VERTEX \"v1\" SET name = \"x\" WHERE arr[0] > 1";
        let stmt = parse(query);
        assert!(normalize_shape(&stmt).is_none());
    }

    #[test]
    fn skip_non_dml() {
        let stmt = parse("MATCH (n:person) RETURN n");
        assert!(normalize_shape(&stmt).is_none());
    }

    #[test]
    fn normalizes_update_vertex() {
        let query = "UPDATE VERTEX \"v1\" SET name = \"x\"";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "UPDATE VERTEX $__dml_0 SET name = $__dml_1"
        );
        assert_eq!(shape.values.len(), 2);
    }

    #[test]
    fn normalizes_update_edge_long_form() {
        let query = "UPDATE EDGE OF works_at FROM \"p1\" TO \"p2\" @1 SET salary = 100";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "UPDATE EDGE OF works_at FROM $__dml_0 TO $__dml_1 @$__dml_2 SET salary = $__dml_3"
        );
    }

    #[test]
    fn normalizes_upsert_edge_short_form() {
        let query = "UPSERT EDGE \"p1\" -> \"p2\" @1 OF works_at SET salary = 100";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "UPSERT EDGE $__dml_0 -> $__dml_1 @$__dml_2 OF works_at SET salary = $__dml_3"
        );
    }

    #[test]
    fn normalizes_update_with_where() {
        let query = "UPDATE VERTEX \"v1\" SET name = \"x\" WHERE age > 30";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "UPDATE VERTEX $__dml_0 SET name = $__dml_1 WHERE (age > $__dml_2)"
        );
    }

    #[test]
    fn update_reparse_matches_target() {
        let query = "UPDATE VERTEX \"v1\" SET name = \"x\" WHERE age > 30";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        let reparsed = parse(&shape.normalized_text);
        match reparsed {
            Stmt::Update(update) => {
                assert!(matches!(update.target, UpdateTarget::Vertex(_)));
                assert!(update.where_clause.is_some());
            }
            _ => panic!("reparse should produce UPDATE"),
        }
    }

    #[test]
    fn update_with_yield_is_skipped() {
        let query = "UPSERT VERTEX ON person SET name = \"x\" WHERE id(vid) == \"v1\" YIELD name";
        let stmt = parse(query);
        assert!(normalize_shape(&stmt).is_none());
    }

    #[test]
    fn normalizes_delete_vertex_list() {
        let query = "DELETE VERTEX \"v1\", \"v2\"";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(shape.normalized_text, "DELETE VERTEX $__dml_0, $__dml_1");
    }

    #[test]
    fn normalizes_delete_edge() {
        let query = "DELETE EDGE works_at \"s1\" -> \"d1\" @0";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "DELETE EDGE works_at $__dml_0 -> $__dml_1 @$__dml_2"
        );
    }

    #[test]
    fn normalizes_delete_tag_with_edge() {
        let query = "DELETE TAG * FROM \"v1\" WITH EDGE";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "DELETE TAG * FROM $__dml_0 WITH EDGE"
        );
    }

    #[test]
    fn normalizes_merge_node_with_properties() {
        let query = "MERGE (n:person {name: \"a\"}) ON CREATE SET age = 1";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "MERGE (n:person {name: $__dml_0}) ON CREATE SET age = $__dml_1"
        );
    }

    #[test]
    fn normalizes_merge_path() {
        let query = "MERGE (a:person)-[:knows]->(b:person) ON MATCH SET score = 5";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "MERGE (a:person)-[:knows]->(b:person) ON MATCH SET score = $__dml_0"
        );
    }

    #[test]
    fn normalizes_set_statement() {
        let query = "SET name = \"x\", age = 30";
        let stmt = parse(query);
        let shape = normalize_shape(&stmt).expect("shape should normalize");
        assert_eq!(shape.normalized_text, "SET name = $__dml_0, age = $__dml_1");
    }

    #[test]
    fn remove_without_literals_is_skipped() {
        let query = "REMOVE v.name, v.age";
        let stmt = parse(query);
        assert!(normalize_shape(&stmt).is_none());
    }

    #[test]
    fn skips_non_roundtrippable_binary_operators() {
        use crate::core::types::operators::BinaryOperator;
        let cases = [
            BinaryOperator::In,
            BinaryOperator::NotIn,
            BinaryOperator::Subscript,
            BinaryOperator::Attribute,
            BinaryOperator::JsonGet,
            BinaryOperator::JsonGetText,
            BinaryOperator::JsonPathGet,
            BinaryOperator::JsonPathGetText,
        ];
        for op in cases {
            let expr = Expression::binary(Expression::variable("a"), op, Expression::variable("b"));
            let mut out = String::new();
            let mut values = Vec::new();
            assert!(
                render_expr(&mut out, &mut values, &expr).is_none(),
                "op {op:?} should be excluded"
            );
        }
    }
}

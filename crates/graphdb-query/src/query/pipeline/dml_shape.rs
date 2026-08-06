//! DML shape normalization for plan-cache reuse.
//!
//! Same-shape DML statements (e.g. 10000 identical INSERT VERTEX templates
//! that only differ in literal values) should compile their physical plan
//! once and reuse it from the plan cache, binding per-statement literal
//! values at execution time.
//!
//! This module re-renders an INSERT statement as a canonical template in
//! which every literal value position is replaced by a `$__dml_N` parameter
//! slot, producing a normalized query text (identical across same-shape
//! statements) together with the ordered literal values used for
//! execution-time binding.

use crate::core::types::expr::ContextualExpression;
use crate::core::Value;
use crate::query::parser::ast::{InsertStmt, InsertTarget, Stmt};

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

/// Normalize an INSERT statement, returning the canonical template text and
/// the ordered literal values.
///
/// Returns `None` when the statement is not shape-cacheable (e.g. a value
/// position is not a top-level literal).
pub fn normalize_dml(stmt: &Stmt) -> Option<DmlShape> {
    let mut values = Vec::new();
    let normalized_text = render_insert(stmt.as_insert()?, &mut values)?;
    if values.is_empty() {
        return None;
    }
    Some(DmlShape {
        normalized_text,
        values,
    })
}

/// Whether a statement is a candidate for DML shape normalization.
pub fn is_dml_shape_candidate(stmt: &Stmt) -> bool {
    matches!(stmt, Stmt::Insert(_))
}

/// Render a canonical INSERT template, appending literal values to `values`.
fn render_insert(insert: &InsertStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("INSERT ");
    match &insert.target {
        InsertTarget::Vertices { tags, values: rows } => {
            out.push_str("VERTEX ");
            if insert.if_not_exists {
                out.push_str("IF NOT EXISTS ");
            }
            for (index, tag) in tags.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&tag.tag_name);
                if !tag.prop_names.is_empty() {
                    out.push('(');
                    out.push_str(&tag.prop_names.join(", "));
                    out.push(')');
                }
            }
            out.push_str(" VALUES ");
            for (row_index, row) in rows.iter().enumerate() {
                if row_index > 0 {
                    out.push_str(", ");
                }
                push_param(&mut out, values, &row.vid)?;
                out.push_str(": ");
                for (group_index, group) in row.tag_values.iter().enumerate() {
                    if group_index > 0 {
                        out.push_str(": ");
                    }
                    out.push('(');
                    for (value_index, value) in group.iter().enumerate() {
                        if value_index > 0 {
                            out.push_str(", ");
                        }
                        push_param(&mut out, values, value)?;
                    }
                    out.push(')');
                }
            }
        }
        InsertTarget::Edge {
            edge_name,
            prop_names,
            edges,
        } => {
            out.push_str("EDGE ");
            if insert.if_not_exists {
                out.push_str("IF NOT EXISTS ");
            }
            out.push_str(edge_name);
            if !prop_names.is_empty() {
                out.push('(');
                out.push_str(&prop_names.join(", "));
                out.push(')');
            }
            out.push_str(" VALUES ");
            for (edge_index, (src, dst, rank, properties)) in edges.iter().enumerate() {
                if edge_index > 0 {
                    out.push_str(", ");
                }
                push_param(&mut out, values, src)?;
                out.push_str(" -> ");
                push_param(&mut out, values, dst)?;
                if let Some(rank) = rank {
                    out.push_str(" @");
                    push_param(&mut out, values, rank)?;
                }
                if !properties.is_empty() {
                    out.push_str(": (");
                    for (value_index, property) in properties.iter().enumerate() {
                        if value_index > 0 {
                            out.push_str(", ");
                        }
                        push_param(&mut out, values, property)?;
                    }
                    out.push(')');
                }
            }
        }
    }
    Some(out)
}

/// Emit a `$__dml_N` placeholder and record the literal value.
fn push_param(out: &mut String, values: &mut Vec<Value>, expr: &ContextualExpression) -> Option<()> {
    let value = expr.expression()?.inner().as_literal()?.clone();
    let index = values.len();
    out.push('$');
    out.push_str(DML_PARAM_PREFIX);
    out.push_str(&index.to_string());
    values.push(value);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let shape = normalize_dml(&stmt).expect("shape should normalize");
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
        let shape = normalize_dml(&stmt).expect("shape should normalize");
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
        let shape_a = normalize_dml(&a).expect("shape a");
        let shape_b = normalize_dml(&b).expect("shape b");
        assert_eq!(shape_a.normalized_text, shape_b.normalized_text);
    }

    #[test]
    fn normalized_text_reparses_with_parameters() {
        let query = "INSERT VERTEX person(name, age) VALUES \"p00001\": (\"Alice\", 30)";
        let stmt = parse(query);
        let shape = normalize_dml(&stmt).expect("shape should normalize");
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
        let query = "INSERT VERTEX IF NOT EXISTS person(name) VALUES \"p1\": (\"A\"), \"p2\": (\"B\")";
        let stmt = parse(query);
        let shape = normalize_dml(&stmt).expect("shape should normalize");
        assert_eq!(
            shape.normalized_text,
            "INSERT VERTEX IF NOT EXISTS person(name) VALUES $__dml_0: ($__dml_1), $__dml_2: ($__dml_3)"
        );
        assert_eq!(shape.values.len(), 4);
    }

    #[test]
    fn skips_non_literal_value() {
        let query = "INSERT VERTEX user(id, name) VALUES \"u1\": (upper(\"bob\"), 1)";
        let stmt = parse(query);
        assert!(normalize_dml(&stmt).is_none());
    }

    #[test]
    fn skip_non_dml() {
        let stmt = parse("MATCH (n:person) RETURN n");
        assert!(!is_dml_shape_candidate(&stmt));
        assert!(normalize_dml(&stmt).is_none());
    }

    #[test]
    fn skips_update_and_delete() {
        let update = parse("UPDATE VERTEX \"v1\" SET name = \"x\"");
        assert!(!is_dml_shape_candidate(&update));
        assert!(normalize_dml(&update).is_none());
        let delete = parse("DELETE VERTEX \"v1\"");
        assert!(!is_dml_shape_candidate(&delete));
        assert!(normalize_dml(&delete).is_none());
    }
}

use super::*;

#[test]
fn test_parse_simple_expression() {
    let input = "1 + 2 * 3";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok());
    let parse_result = result.expect("Simple expression parsing should succeed");
    assert!(matches!(parse_result.expr, Expression::Binary { .. }));
}

#[test]
fn test_parse_struct_literal_and_field_access() {
    // STRUCT literal folds into a constant Struct value.
    let input = "STRUCT{name: 'x', addr: STRUCT{city: 'sh', geo: STRUCT{lat: 1.0, lon: 2.0}}}";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("STRUCT literal must parse");
    match result.expr {
        Expression::Literal(Value::Struct(s)) => {
            assert_eq!(s.fields.len(), 2);
            assert_eq!(s.fields[0].0, "name");
            assert!(
                matches!(s.fields[1].1, Value::Struct(_)),
                "nested STRUCT must fold"
            );
        }
        other => panic!("expected STRUCT literal, got {:?}", other),
    }

    // `addr.city` chains as StructField.
    let input = "p.addr.city";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("field access must parse");
    match result.expr {
        Expression::StructField { base, field } => {
            assert_eq!(field, "city");
            assert!(matches!(*base, Expression::Property { .. }));
        }
        other => panic!("expected StructField, got {:?}", other),
    }

    // Chained field access nests StructField on StructField.
    let input = "p.addr.geo.lat";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("chained field access must parse");
    match result.expr {
        Expression::StructField { base, field } => {
            assert_eq!(field, "lat");
            assert!(matches!(*base, Expression::StructField { .. }));
        }
        other => panic!("expected nested StructField, got {:?}", other),
    }

    // A single dot on a bare variable stays a property access.
    let input = "p.name";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("property access must parse");
    assert!(matches!(result.expr, Expression::Property { .. }));
}

#[test]
fn test_parse_array_literal_and_subscript() {
    // ARRAY literal folds into a constant Array value.
    let input = "ARRAY[1, 2, 3]";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("ARRAY literal must parse");
    match result.expr {
        Expression::Literal(Value::Array(a)) => {
            assert_eq!(
                a.values,
                vec![Value::BigInt(1), Value::BigInt(2), Value::BigInt(3)]
            );
        }
        other => panic!("expected ARRAY literal, got {:?}", other),
    }

    // `arr[0]` stays a Subscript.
    let input = "arr[0]";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx).expect("subscript must parse");
    assert!(matches!(result.expr, Expression::Subscript { .. }));
}

#[test]
fn test_parse_json_path_get() {
    // Regression: peek_token used to return the current token, so the
    // `expr -> 'key'` JSON access postfix never fired.
    for input in ["m->'key'", "m->>\"key\"", "m#>'a.b'", "m#>>\"a.b\""] {
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(
            result.is_ok(),
            "parse failed for {input}: {:?}",
            result.err()
        );
        let parse_result = result.expect("JSON access parsing should succeed");
        assert!(matches!(parse_result.expr, Expression::Binary { .. }));
    }
}

#[test]
fn test_parse_in_subquery() {
    // Regression: peek_token used to return the current token, so
    // `x IN { subquery }` was never recognized and the tail was dropped.
    let input = "t.name IN { MATCH (p:person) RETURN p.name }";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let parse_result = result.expect("IN subquery parsing should succeed");
    match parse_result.expr {
        Expression::In {
            expr,
            subquery,
            negated,
        } => {
            assert!(!negated);
            assert!(matches!(*expr, Expression::Property { .. }));
            assert_eq!(subquery.patterns.len(), 1);
            assert!(subquery.return_expr.is_some());
        }
        other => panic!("expected Expression::In, got {:?}", other),
    }
}

#[test]
fn test_parse_not_in_subquery() {
    // Regression: `NOT IN` lexes as a single NotIn token that the parser
    // used to drop, silently discarding the whole subquery.
    let input = "t.name NOT IN { MATCH (p:person) RETURN p.name }";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let parse_result = result.expect("NOT IN subquery parsing should succeed");
    match parse_result.expr {
        Expression::In {
            expr,
            subquery,
            negated,
        } => {
            assert!(negated);
            assert!(matches!(*expr, Expression::Property { .. }));
            assert_eq!(subquery.patterns.len(), 1);
            assert!(subquery.return_expr.is_some());
        }
        other => panic!("expected negated Expression::In, got {:?}", other),
    }
}

#[test]
fn test_parse_parenthesized_expression() {
    let input = "(1 + 2) * 3";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok());
    let parse_result = result.expect("Parsing a bracketed expression should succeed");
    assert!(matches!(parse_result.expr, Expression::Binary { .. }));
}

#[test]
fn test_parse_exists_with_where_clause() {
    // Regression: parse_pattern_string used to consume the WHERE / RETURN
    // terminators, making `EXISTS { MATCH ... WHERE ... }` fail with
    // "Expected RBrace, found Identifier(...)".
    let input = "EXISTS { MATCH (q:person) WHERE q.age > 30 }";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let parse_result = result.expect("EXISTS parsing should succeed");
    match parse_result.expr {
        Expression::Exists { body } => {
            assert_eq!(body.patterns.len(), 1, "one pattern expected");
            assert_eq!(body.patterns[0], "( q : person )");
            assert!(
                body.where_clause.is_some(),
                "WHERE clause must be preserved"
            );
        }
        other => panic!("expected Expression::Exists, got {:?}", other),
    }
}

#[test]
fn test_parse_exists_with_return_expr() {
    let input = "EXISTS { MATCH (q:person) WHERE q.age > 30 RETURN q.name }";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let parse_result = result.expect("EXISTS parsing should succeed");
    match parse_result.expr {
        Expression::Exists { body } => {
            assert_eq!(body.patterns.len(), 1);
            assert!(body.where_clause.is_some());
            assert!(body.return_expr.is_some(), "RETURN expr must be preserved");
        }
        other => panic!("expected Expression::Exists, got {:?}", other),
    }
}

#[test]
fn test_parse_exists_bare_pattern() {
    let input = "EXISTS { a:person-[:knows]->b:person }";
    let ctx = &mut ParseContext::new(input);
    let result = parse_expression(ctx);
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let parse_result = result.expect("EXISTS parsing should succeed");
    match parse_result.expr {
        Expression::Exists { body } => {
            assert_eq!(body.patterns.len(), 1);
            assert_eq!(
                body.patterns[0],
                "( a : person ) - [ : knows ] -> ( b : person )"
            );
            assert!(body.where_clause.is_none());
        }
        other => panic!("expected Expression::Exists, got {:?}", other),
    }
}

#[test]
fn test_subquery_pattern_round_trip_reparse() {
    // The stored pattern strings must be re-parseable by the traversal
    // parser, which the exists planner relies on to build the subquery
    // plan. Both the parenthesized (MATCH) and the bare forms are
    // canonicalized at parse time.
    for input in [
        "EXISTS { MATCH (q:person) }",
        "EXISTS { a:person-[:knows]->b:person }",
        "EXISTS { MATCH (a:person)-[:knows]->(b:person) WHERE b.age > 18 }",
    ] {
        let ctx = &mut ParseContext::new(input);
        let result = parse_expression(ctx);
        assert!(
            result.is_ok(),
            "parse failed for {input}: {:?}",
            result.err()
        );
        let parse_result = result.expect("EXISTS parsing should succeed");
        let body = match parse_result.expr {
            Expression::Exists { body } => body,
            other => panic!("expected Expression::Exists, got {:?}", other),
        };
        for pattern_str in &body.patterns {
            let ctx = &mut ParseContext::new(pattern_str);
            let mut parser = crate::parser::parsing::traversal_parser::TraversalParser::new();
            let pattern = parser.parse_pattern(ctx);
            assert!(
                pattern.is_ok(),
                "stored pattern `{pattern_str}` must be re-parseable: {:?}",
                pattern.err()
            );
        }
    }
}

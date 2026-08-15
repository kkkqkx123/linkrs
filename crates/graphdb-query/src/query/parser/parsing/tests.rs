use crate::query::parser::ast::stmt::*;
use crate::query::parser::parsing::Parser;

#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::*;

    fn parse_statement(query: &str) -> Result<Stmt, crate::query::parser::core::error::ParseError> {
        let mut parser = Parser::new(query);
        let parser_result = parser.parse()?;
        Ok(parser_result.ast.stmt.clone())
    }

    #[test]
    fn test_insert_edge_basic() {
        let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT EDGE parse should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT EDGE parsing should succeed");
        assert_eq!(stmt.kind(), "INSERT");

        if let Stmt::Insert(insert_stmt) = stmt {
            if let InsertTarget::Edge {
                edge_name, edges, ..
            } = insert_stmt.target
            {
                assert_eq!(edge_name, "KNOWS");
                assert_eq!(edges.len(), 1);
                let (_, _, _, values) = &edges[0];
                assert_eq!(values.len(), 1);
            } else {
                panic!("Expectations for the Edge target");
            }
        } else {
            panic!("The expected Insert statement");
        }
    }

    #[test]
    fn test_insert_edge_with_rank() {
        let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2 @0:('2020-01-01')";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT EDGE with rank Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT EDGE with rank parsing should succeed!");
        assert_eq!(stmt.kind(), "INSERT");

        if let Stmt::Insert(insert_stmt) = stmt {
            if let InsertTarget::Edge {
                edge_name, edges, ..
            } = insert_stmt.target
            {
                assert_eq!(edge_name, "KNOWS");
                assert_eq!(edges.len(), 1);
                let (_, _, rank, _) = &edges[0];
                assert!(rank.is_some(), "The “rank” should definitely be included.");
            } else {
                panic!("Expectations for the Edge target");
            }
        } else {
            panic!("The “Expect” statement is used to specify the expected behavior or output of a system or process. It helps in verifying that the system is functioning as intended by checking whether the actual results match the expected results.");
        }
    }

    #[test]
    fn test_insert_edge_multiple() {
        let query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 2 -> 3:('2021-01-01')";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT Multiple side parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT multiple edge parsing should succeed");
        assert_eq!(stmt.kind(), "INSERT");
    }

    #[test]
    fn test_insert_edge_multiple_properties() {
        let query = "INSERT EDGE KNOWS(since, weight) VALUES 1 -> 2:('2020-01-01', 0.9)";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT EDGE Multiple attribute parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT EDGE multi-attribute parsing should succeed");
        assert_eq!(stmt.kind(), "INSERT");
    }

    #[test]
    fn test_insert_vertex_basic() {
        let query = "INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT VERTEX Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT VERTEX parsing should succeed");
        assert_eq!(stmt.kind(), "INSERT");
    }

    #[test]
    fn test_insert_vertex_multiple() {
        let query = "INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "INSERT Multiple vertex resolution should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("INSERT multiple vertex resolution should succeed");
        assert_eq!(stmt.kind(), "INSERT");
    }

    #[test]
    fn test_delete_edge_basic() {
        let query = "DELETE EDGE KNOWS 1 -> 2";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "DELETE EDGE Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("DELETE EDGE parsing should succeed");
        assert_eq!(stmt.kind(), "DELETE");
    }

    #[test]
    fn test_delete_edge_with_rank() {
        let query = "DELETE EDGE KNOWS 1 -> 2 @0";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "DELETE EDGE with rank Parse should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("DELETE EDGE with rank parsing should succeed");
        assert_eq!(stmt.kind(), "DELETE");
    }

    #[test]
    fn test_delete_edge_multiple() {
        let query = "DELETE EDGE KNOWS 1 -> 2, 2 -> 3";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "DELETE Multiple side parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("DELETE multiple edge parsing should succeed");
        assert_eq!(stmt.kind(), "DELETE");
    }

    #[test]
    fn test_set_property_basic() {
        let query = "SET p.age = 26";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "SET attribute parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("SET attribute parsing should succeed");
        assert_eq!(stmt.kind(), "SET");
    }

    #[test]
    fn test_set_property_multiple() {
        let query = "SET p.age = 26, p.name = 'Alice'";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "SET Multiple attribute parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("SET multiple attribute parsing should succeed");
        assert_eq!(stmt.kind(), "SET");
    }

    #[test]
    fn test_set_property_with_expression() {
        let query = "SET p.age = p.age + 1";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "SET Parsing with expression should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("SET with expression parsing should succeed");
        assert_eq!(stmt.kind(), "SET");
    }

    #[test]
    fn test_update_vertex_basic() {
        let query = "UPDATE 1 SET age = 26";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "UPDATE vertex resolution should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("UPDATE vertex resolution should succeed");
        assert_eq!(stmt.kind(), "UPDATE");
    }

    #[test]
    fn test_delete_vertex_basic() {
        let query = "DELETE VERTEX 1";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "DELETE VERTEX Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("DELETE VERTEX parsing should succeed!");
        assert_eq!(stmt.kind(), "DELETE");
    }

    #[test]
    fn test_find_shortest_path_basic() {
        let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND SHORTEST PATH Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND SHORTEST PATH parsing should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.shortest,
                "It should be the query for the shortest path."
            );
            assert!(
                find_path_stmt.weight_expression.is_none(),
                "Expression with no right to be evaluated (or expressed)"
            );
        } else {
            panic!("Expectations for the FindPath statement");
        }
    }

    #[test]
    fn test_find_weighted_shortest_path() {
        let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect WEIGHT weight";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "With rights FIND SHORTEST PATH Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("Parsing the FIND SHORTEST PATH with rights should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.shortest,
                "It should be the shortest path query."
            );
            assert_eq!(
                find_path_stmt.weight_expression,
                Some("weight".to_string()),
                "There should be a weight expression."
            );
        } else {
            panic!("The expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_find_weighted_shortest_path_with_ranking() {
        let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect WEIGHT ranking";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND SHORTEST PATH parsing with ranking weights should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND SHORTEST PATH parsing using ranking weights should succeed");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.shortest,
                "It should be the query for the shortest path."
            );
            assert_eq!(
                find_path_stmt.weight_expression,
                Some("ranking".to_string()),
                "There should be an expression for the ranking weights."
            );
        } else {
            panic!("Expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_find_all_paths() {
        let query = "FIND ALL PATH FROM 1 TO 2 OVER connect";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND ALL PATH Parsing should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND ALL PATH parsing should succeed");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                !find_path_stmt.shortest,
                "It should refer to all path queries."
            );
        } else {
            panic!("Expectations for the FindPath statement");
        }
    }

    #[test]
    fn test_find_shortest_path_with_steps() {
        let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect UPTO 5 STEPS";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND SHORTEST PATH parsing with step limit should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND SHORTEST PATH parsing with a step limit should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.shortest,
                "It should be the query for the shortest path."
            );
            assert_eq!(
                find_path_stmt.max_steps,
                Some(5),
                "There should be a maximum number of steps, which is 5."
            );
        } else {
            panic!("Expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_find_path_with_loop() {
        let query = "FIND ALL PATH WITH LOOP FROM 1 TO 2 OVER connect";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND PATH parsing with WITH LOOP should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND PATH parsing with WITH LOOP should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.with_loop,
                "Self-loop edges should be allowed."
            );
            assert!(
                !find_path_stmt.with_cycle,
                "By default, loops are not allowed."
            );
        } else {
            panic!("Expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_find_path_with_cycle() {
        let query = "FIND ALL PATH WITH CYCLE FROM 1 TO 2 OVER connect";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND PATH parsing with WITH CYCLE should succeed: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND PATH parsing with WITH CYCLE should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                !find_path_stmt.with_loop,
                "By default, self-looping edges are not allowed."
            );
            assert!(find_path_stmt.with_cycle, "The loop should be allowed.");
        } else {
            panic!("The expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_find_path_with_loop_and_cycle() {
        let query = "FIND ALL PATH WITH LOOP WITH CYCLE FROM 1 TO 2 OVER connect";
        let result = parse_statement(query);
        assert!(
            result.is_ok(),
            "FIND PATH with WITH LOOP WITH CYCLE should parse successfully: {:?}",
            result.err()
        );

        let stmt = result.expect("FIND PATH parsing with WITH LOOP WITH CYCLE should succeed!");
        assert_eq!(stmt.kind(), "FIND PATH");

        if let Stmt::FindPath(find_path_stmt) = stmt {
            assert!(
                find_path_stmt.with_loop,
                "Self-loop edges should be allowed."
            );
            assert!(find_path_stmt.with_cycle, "The circuit should be allowed to operate (i.e., its operation should be permitted).");
        } else {
            panic!("The expectation for the FindPath statement");
        }
    }

    #[test]
    fn test_error_recovery_collects_multiple_errors() {
        let query = "MATCH (n:Person RETURN n";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(result.is_err() || parser.has_errors());
    }

    #[test]
    fn test_trailing_tokens_are_rejected() {
        // A statement must consume the whole input; trailing garbage is a
        // syntax error instead of being silently truncated.
        let mut parser = Parser::new("COMMIT junk");
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "statement should still parse structurally: {:?}",
            result.err()
        );
        assert!(parser.has_errors(), "trailing token must be reported");
        let result = result.expect("statement should parse");
        assert!(
            matches!(result.ast.stmt(), Stmt::CommitTransaction(_)),
            "leading COMMIT should still parse"
        );
    }

    #[test]
    fn test_trailing_semicolon_is_allowed() {
        let mut parser = Parser::new("MATCH (v) RETURN v;");
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "trailing semicolon parses: {:?}",
            result.err()
        );
        assert!(!parser.has_errors(), "trailing semicolon is not an error");
    }

    #[test]
    fn test_error_recovery_stops_at_limit() {
        let query = "INVALID INVALID INVALID INVALID INVALID INVALID";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(result.is_err());
    }

    #[test]
    fn test_clause_recovery_single_error_continues() {
        // The WHERE expression is malformed; recovery must synchronize to the
        // RETURN clause keyword and still parse the RETURN clause.
        let query = "MATCH (v) WHERE v.age > RETURN v";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "statement should parse structurally after clause recovery: {:?}",
            result.err()
        );

        let errors = parser.take_errors();
        assert_eq!(errors.len(), 1, "exactly one clause error expected");

        let error = &errors.errors[0];
        assert!(error.recovered, "clause error must be marked as recovered");
        assert!(
            error.message.contains("Return"),
            "diagnostics should name the offending token: {}",
            error.message
        );

        // The recovered statement keeps the following clause.
        if let Ok(parser_result) = result {
            if let Stmt::Match(stmt) = parser_result.ast.stmt.clone() {
                assert!(
                    stmt.return_clause.is_some(),
                    "RETURN clause must survive WHERE recovery"
                );
            } else {
                panic!("expected Match statement");
            }
        }
    }

    #[test]
    fn test_clause_recovery_multiple_errors() {
        // Two clause-level errors in one statement: a broken WHERE expression
        // and a LIMIT without an integer. Both must be collected.
        let query = "MATCH (v) WHERE v.age > RETURN v LIMIT";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "statement should parse structurally after clause recovery: {:?}",
            result.err()
        );

        let errors = parser.take_errors();
        assert_eq!(errors.len(), 2, "both clause errors must be recorded");
        assert!(
            errors.errors.iter().all(|e| e.recovered),
            "all clause errors must be marked as recovered"
        );
    }

    #[test]
    fn test_clause_recovery_across_union_operands() {
        // A compound statement whose operands each carry a broken WHERE
        // clause: every clause error is reported in a single pass.
        let query = "MATCH (v) WHERE v.age > RETURN v UNION MATCH (v) WHERE v.age > RETURN v";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "statement should parse structurally after clause recovery: {:?}",
            result.err()
        );

        let errors = parser.take_errors();
        assert_eq!(errors.len(), 2, "one clause error per union operand");
        assert!(
            errors.errors.iter().all(|e| e.recovered),
            "all clause errors must be marked as recovered"
        );
    }

    #[test]
    fn test_clause_recovery_stops_at_limit() {
        // More clause errors than RECOVERY_LIMIT (5): parsing must abort.
        let broken = "MATCH (v) WHERE v.age > RETURN v UNION ";
        let query = format!(
            "{}{}{}{}{}{}",
            broken, broken, broken, broken, broken, broken
        );
        let mut parser = Parser::new(&query);
        let result = parser.parse();
        assert!(result.is_err(), "recovery limit must abort parsing");
    }

    #[test]
    fn test_clause_recovery_expected_tokens_attached() {
        // LIMIT without an integer literal: the recorded error must carry the
        // expected token candidates.
        let query = "MATCH (v) RETURN v LIMIT";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "statement should parse structurally after clause recovery: {:?}",
            result.err()
        );

        let errors = parser.take_errors();
        assert_eq!(errors.len(), 1);
        let error = &errors.errors[0];
        assert!(
            !error.expected_tokens.is_empty(),
            "expected token candidates must be attached"
        );
        assert!(error.recovered);
    }

    #[test]
    fn test_clause_recovery_normal_path_unchanged() {
        // Valid clauses must not trigger recovery or emit diagnostics.
        let query = "MATCH (v) WHERE v.age > 30 RETURN v ORDER BY v.age LIMIT 10";
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(result.is_ok(), "valid query parses: {:?}", result.err());
        assert!(
            !parser.has_errors(),
            "valid query must not produce any diagnostics"
        );

        if let Ok(parser_result) = result {
            if let Stmt::Match(stmt) = parser_result.ast.stmt.clone() {
                assert!(stmt.where_clause.is_some());
                let return_clause = stmt.return_clause.as_ref().expect("RETURN present");
                assert!(return_clause.order_by.is_some());
                assert_eq!(return_clause.limit.as_ref().map(|l| l.count), Some(10));
            } else {
                panic!("expected Match statement");
            }
        }
    }

    #[test]
    fn test_extension_registry_empty() {
        use crate::query::parser::parsing::ExtensionRegistry;
        let registry = ExtensionRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn test_extension_registry_register() {
        use crate::core::types::expr::contextual::ContextualExpression;
        use crate::query::parser::core::error::ParseError;
        use crate::query::parser::core::error::ParseErrorKind;
        use crate::query::parser::parsing::parse_context::ParseContext;
        use crate::query::parser::parsing::ExtensionParseResult;
        use crate::query::parser::parsing::ExtensionRegistry;
        use crate::query::parser::parsing::ParserExtension;
        use crate::query::parser::TokenKind;

        struct TestExtension;

        impl ParserExtension for TestExtension {
            fn name(&self) -> &str {
                "test"
            }

            fn handled_statement_tokens(&self) -> &[TokenKind] {
                &[]
            }

            fn handled_expression_tokens(&self) -> &[TokenKind] {
                &[]
            }

            fn try_parse_statement(&self, _ctx: &mut ParseContext) -> ExtensionParseResult {
                ExtensionParseResult::NotMatched
            }

            fn try_parse_expression(
                &self,
                _ctx: &mut ParseContext,
            ) -> Result<ContextualExpression, ParseError> {
                Err(ParseError::new(
                    ParseErrorKind::SyntaxError,
                    "not implemented",
                    crate::core::types::Position::new(0, 0),
                ))
            }
        }

        let mut registry = ExtensionRegistry::new();
        registry.register(Box::new(TestExtension));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }
}

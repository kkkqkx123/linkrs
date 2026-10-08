//! Tests for statement dispatch and pipeline parsing via `StmtParser`.

use super::*;
use crate::parser::ast::stmt::*;

fn create_parser_context<'a>(input: &'a str) -> ParseContext<'a> {
    ParseContext::new(input)
}

#[test]
fn test_parse_match_statement() {
    let mut ctx = create_parser_context("MATCH (n:Person) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "MATCH parse failure: {:?}", result.err());
}

#[test]
fn test_parse_match_binary_join_hint() {
    let mut ctx = create_parser_context(
        "MATCH (a)-[e1]->(b), (a)-[e2]->(c) USING JOIN BINARY(e1, e2) RETURN a",
    );
    let stmt = StmtParser::parse_statement(&mut ctx).expect("hint must parse");
    let crate::parser::ast::Stmt::Match(m) = stmt else {
        panic!("expected MATCH");
    };
    assert_eq!(
        m.join_hint,
        Some(crate::parser::ast::JoinHintAst::Binary {
            left: "e1".to_string(),
            right: "e2".to_string(),
        })
    );
    assert_eq!(m.patterns.len(), 2);
}

#[test]
fn test_parse_match_multiway_join_hint() {
    let mut ctx = create_parser_context(
        "MATCH (a)-[e1]->(b), (a)-[e2]->(c) USING JOIN MULTIWAY(e1, e2) RETURN a",
    );
    let stmt = StmtParser::parse_statement(&mut ctx).expect("hint must parse");
    let crate::parser::ast::Stmt::Match(m) = stmt else {
        panic!("expected MATCH");
    };
    assert_eq!(
        m.join_hint,
        Some(crate::parser::ast::JoinHintAst::Multiway {
            probe: "e1".to_string(),
            builds: vec!["e2".to_string()],
        })
    );
}

#[test]
fn test_parse_match_without_hint_has_none() {
    let mut ctx = create_parser_context("MATCH (a)-[e1]->(b) RETURN a");
    let stmt = StmtParser::parse_statement(&mut ctx).expect("must parse");
    let crate::parser::ast::Stmt::Match(m) = stmt else {
        panic!("expected MATCH");
    };
    assert_eq!(m.join_hint, None);
}

#[test]
fn test_parse_match_using_as_variable_still_parses() {
    let mut ctx = create_parser_context("MATCH (using) RETURN using");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "plain `using` variable must parse");
}

#[test]
fn test_parse_match_join_hint_rejects_bad_shape() {
    let mut ctx = create_parser_context("MATCH (a)-[e1]->(b) USING JOIN FOO(e1) RETURN a");
    assert!(StmtParser::parse_statement(&mut ctx).is_err());
    let mut ctx = create_parser_context("MATCH (a)-[e1]->(b) USING JOIN MULTIWAY(e1) RETURN a");
    assert!(StmtParser::parse_statement(&mut ctx).is_err());
}

#[test]
fn test_parse_go_statement() {
    let mut ctx = create_parser_context("GO 1 STEP FROM \"player100\" OVER follow");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "GO parse failure: {:?}", result.err());
}

#[test]
fn test_parse_create_tag_statement() {
    let mut ctx = create_parser_context("CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "CREATE TAG Parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_create_tag_with_composite_types() {
    let mut ctx = create_parser_context(
        "CREATE TAG Person (id INT, \
         addr STRUCT<city STRING, street STRING, geo STRUCT<lat DOUBLE, lon DOUBLE>>, \
         coords ARRAY<DOUBLE>(3), \
         tags ARRAY<STRING>)",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    let stmt = result.expect("composite type DDL must parse");
    let crate::parser::ast::Stmt::Create(create) = stmt else {
        panic!("expected Create statement");
    };
    let crate::parser::ast::CreateTarget::Tag {
        properties: props, ..
    } = create.target
    else {
        panic!("expected Tag creation");
    };
    let props: Vec<_> = props
        .iter()
        .map(|p| (p.name.clone(), p.data_type.clone()))
        .collect();
    use linkrs_core::{ArrayTypeInfo, DataType, StructTypeInfo};
    use std::sync::Arc;
    assert_eq!(props[0].0, "id");
    assert_eq!(props[0].1, DataType::Int);
    assert_eq!(
        props[1].1,
        DataType::Struct(Arc::new(StructTypeInfo::new(vec![
            ("city".into(), DataType::String),
            ("street".into(), DataType::String),
            (
                "geo".to_string(),
                DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                    ("lat".into(), DataType::Double),
                    ("lon".into(), DataType::Double),
                ]))),
            ),
        ])))
    );
    assert_eq!(
        props[2].1,
        DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3))))
    );
    assert_eq!(
        props[3].1,
        DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::String, None)))
    );
}

#[test]
fn test_parse_create_tag_composite_nesting_limit() {
    let mut ddl = String::from("CREATE TAG Deep (a ARRAY<");
    for _ in 0..17 {
        ddl.push_str("ARRAY<");
    }
    ddl.push_str("INT");
    for _ in 0..17 {
        ddl.push('>');
    }
    ddl.push_str(">)");
    let mut ctx = create_parser_context(&ddl);
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_err(),
        "over-nested composite type must be rejected"
    );
}

#[test]
fn test_parse_insert_vertex_statement() {
    let mut ctx =
        create_parser_context("INSERT VERTEX Person(name, age) VALUES \"player100\":(\"Tom\", 18)");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "INSERT VERTEX parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_delete_vertex_statement() {
    let mut ctx = create_parser_context("DELETE VERTEX person FROM \"player100\"");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "DELETE VERTEX parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Delete(delete_stmt)) = result {
        if let crate::parser::ast::DeleteTarget::Vertices { tag, vids } = delete_stmt.target {
            assert_eq!(tag, "person");
            assert_eq!(vids.len(), 1);
        } else {
            panic!("Expected vertex delete target");
        }
    } else {
        panic!("Expected DELETE statement");
    }
}

#[test]
fn test_parse_use_statement() {
    let mut ctx = create_parser_context("USE test_space");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "USE Parse failure: {:?}", result.err());

    if let Ok(Stmt::Use(stmt)) = result {
        assert_eq!(stmt.space, "test_space");
    } else {
        panic!("Expected Use statement");
    }
}

#[test]
fn test_parse_show_spaces_statement() {
    let mut ctx = create_parser_context("SHOW SPACES");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW SPACES parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_create_space_statement_parses() {
    let mut ctx = create_parser_context("CREATE SPACE IF NOT EXISTS test_space");
    let result = StmtParser::parse_statement(&mut ctx);

    assert!(
        result.is_ok(),
        "CREATE SPACE Parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::Create(stmt)) = result {
        match &stmt.target {
            CreateTarget::Space { name, vid_type, .. } => {
                assert_eq!(name, "test_space");
                assert_eq!(vid_type, "INT64");
            }
            _ => panic!(
                "Expect Space to create a goal and actually get {:?}",
                stmt.target
            ),
        }
        assert!(stmt.if_not_exists);
    } else {
        panic!("The expected Create statement");
    }
}

#[test]
fn test_create_space_with_params_parses() {
    let mut ctx = create_parser_context("CREATE SPACE test_space(vid_type=FIXEDSTRING32)");
    let result = StmtParser::parse_statement(&mut ctx);

    assert!(
        result.is_ok(),
        "CREATE SPACE with params failed to parse: {:?}",
        result.err()
    );

    if let Ok(Stmt::Create(stmt)) = result {
        match &stmt.target {
            CreateTarget::Space { name, vid_type, .. } => {
                assert_eq!(name, "test_space");
                assert_eq!(vid_type, "FIXEDSTRING32");
            }
            _ => panic!(
                "Expect Space to create a goal and actually get {:?}",
                stmt.target
            ),
        }
    } else {
        panic!("The expected Create statement");
    }
}

#[test]
fn test_parse_explain_statement() {
    let mut ctx = create_parser_context("EXPLAIN MATCH (n) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "EXPLAIN Parse failure: {:?}", result.err());

    if let Ok(Stmt::Explain(stmt)) = result {
        assert!(matches!(stmt.format, ExplainFormat::Table));
    } else {
        panic!("Expected Explain statement");
    }
}

#[test]
fn test_parse_explain_with_format() {
    let mut ctx = create_parser_context("EXPLAIN FORMAT = DOT MATCH (n) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "EXPLAIN FORMAT failed to parse: {:?}",
        result.err()
    );

    if let Ok(Stmt::Explain(stmt)) = result {
        assert!(matches!(stmt.format, ExplainFormat::Dot));
        assert!(!stmt.analyze);
    } else {
        panic!("Expected Explain statement");
    }
}

#[test]
fn test_parse_explain_analyze_statement() {
    let mut ctx = create_parser_context("EXPLAIN ANALYZE MATCH (n) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "EXPLAIN ANALYZE failed to parse: {:?}",
        result.err()
    );

    if let Ok(Stmt::Explain(stmt)) = result {
        assert!(stmt.analyze);
    } else {
        panic!("Expected EXPLAIN ANALYZE statement");
    }
}

#[test]
fn test_parse_explain_analyze_with_format() {
    let mut ctx = create_parser_context("EXPLAIN ANALYZE FORMAT = DOT MATCH (n) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "EXPLAIN ANALYZE FORMAT failed to parse: {:?}",
        result.err()
    );

    if let Ok(Stmt::Explain(stmt)) = result {
        assert!(stmt.analyze);
        assert!(matches!(stmt.format, ExplainFormat::Dot));
    } else {
        panic!("Expected EXPLAIN ANALYZE statement");
    }
}

#[test]
fn test_parse_profile_statement() {
    let mut ctx = create_parser_context("PROFILE GO FROM \"player100\" OVER follow");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "PROFILE parse failure: {:?}", result.err());

    if let Ok(Stmt::Profile(stmt)) = result {
        assert!(matches!(stmt.format, ExplainFormat::Table));
    } else {
        panic!("Expected Profile statement");
    }
}

#[test]
fn test_parse_analyze_statement() {
    let mut ctx = create_parser_context("ANALYZE");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "ANALYZE parse failure: {:?}", result.err());

    if let Ok(Stmt::Analyze(stmt)) = result {
        assert_eq!(stmt.space, None);
    } else {
        panic!("Expected Analyze statement");
    }
}

#[test]
fn test_parse_analyze_space_statement() {
    let mut ctx = create_parser_context("ANALYZE SPACE basketball");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "ANALYZE SPACE parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::Analyze(stmt)) = result {
        assert_eq!(stmt.space.as_deref(), Some("basketball"));
    } else {
        panic!("Expected Analyze statement");
    }
}

#[test]
fn test_parse_profile_with_format() {
    let mut ctx = create_parser_context("PROFILE FORMAT = TABLE MATCH (n) RETURN n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "PROFILE FORMAT failed to parse: {:?}",
        result.err()
    );

    if let Ok(Stmt::Profile(stmt)) = result {
        assert!(matches!(stmt.format, ExplainFormat::Table));
    } else {
        panic!("Expected Profile statement");
    }
}

#[test]
fn test_parse_group_by_statement() {
    let mut ctx = create_parser_context("GROUP BY category YIELD category");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "GROUP BY Parse failure: {:?}", result.err());

    if let Ok(Stmt::GroupBy(stmt)) = result {
        assert_eq!(stmt.group_items.len(), 1);
        assert_eq!(stmt.yield_clause.items.len(), 1);
        assert!(stmt.having_clause.is_none());
    } else {
        panic!("Expected GroupBy statement");
    }
}

#[test]
fn test_parse_group_by_multiple_items() {
    let mut ctx = create_parser_context("GROUP BY category, type YIELD category, type");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "GROUP BY multiple field parsing failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::GroupBy(stmt)) = result {
        assert_eq!(stmt.group_items.len(), 2);
        assert_eq!(stmt.yield_clause.items.len(), 2);
    } else {
        panic!("Expected GroupBy statement");
    }
}

#[test]
fn test_parse_show_sessions() {
    let mut ctx = create_parser_context("SHOW SESSIONS");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW SESSIONS Parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::ShowSessions(_)) = result {
    } else {
        panic!("Expected ShowSessions statement");
    }
}

#[test]
fn test_parse_show_queries() {
    let mut ctx = create_parser_context("SHOW QUERIES");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW QUERIES Parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::ShowQueries(_)) = result {
    } else {
        panic!("Expected ShowQueries statement");
    }
}

#[test]
fn test_parse_kill_query() {
    let mut ctx = create_parser_context("KILL QUERY 123, 456");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "KILL QUERY Parsing failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::KillQuery(stmt)) = result {
        assert_eq!(stmt.session_id, 123);
        assert_eq!(stmt.plan_id, 456);
    } else {
        panic!("Expected KillQuery statement");
    }
}

#[test]
fn test_parse_begin_transaction_access_modes() {
    let mut ctx = create_parser_context("BEGIN TRANSACTION");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "BEGIN TRANSACTION parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::BeginTransaction(stmt)) = result {
        assert_eq!(stmt.read_only, None);
    } else {
        panic!("Expected a BeginTransaction statement");
    }

    let mut ctx = create_parser_context("BEGIN READ ONLY");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "BEGIN READ ONLY parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::BeginTransaction(stmt)) = result {
        assert_eq!(stmt.read_only, Some(true));
    } else {
        panic!("Expected a BeginTransaction statement");
    }

    let mut ctx = create_parser_context("BEGIN TRANSACTION READ WRITE");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "BEGIN TRANSACTION READ WRITE parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::BeginTransaction(stmt)) = result {
        assert_eq!(stmt.read_only, Some(false));
    } else {
        panic!("Expected a BeginTransaction statement");
    }

    let mut ctx = create_parser_context("BEGIN READ");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_err(),
        "BEGIN READ should be rejected as an incomplete access mode"
    );
}

#[test]
fn test_parse_show_configs() {
    let mut ctx = create_parser_context("SHOW CONFIGS");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW CONFIGS Parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::ShowConfigs(stmt)) = result {
        assert!(stmt.module.is_none());
    } else {
        panic!("Expected ShowConfigs statement");
    }
}

#[test]
fn test_parse_show_configs_with_module() {
    let mut ctx = create_parser_context("SHOW CONFIGS storage");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW CONFIGS storage Parse failed: {:?}",
        result.err()
    );

    if let Ok(Stmt::ShowConfigs(stmt)) = result {
        assert_eq!(stmt.module, Some("storage".to_string()));
    } else {
        panic!("Expected ShowConfigs statement");
    }
}

#[test]
fn test_parse_update_configs() {
    let mut ctx = create_parser_context("UPDATE CONFIGS max_connections = 100");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::UpdateConfigs(stmt)) = result {
        assert!(stmt.module.is_none());
        assert_eq!(stmt.config_name, "max_connections");
    } else {
        panic!("Expected UpdateConfigs statement");
    }
}

#[test]
fn test_parse_update_configs_with_module() {
    let mut ctx = create_parser_context("UPDATE CONFIGS storage cache_size = 1024");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS storage Parse failed: {:?}",
        result.err()
    );

    if let Ok(Stmt::UpdateConfigs(stmt)) = result {
        assert_eq!(stmt.module, Some("storage".to_string()));
        assert_eq!(stmt.config_name, "cache_size");
    } else {
        panic!("Expected UpdateConfigs statement");
    }
}

#[test]
fn test_parse_assignment_statement() {
    let mut ctx = create_parser_context("$result = GO FROM \"player100\" OVER follow");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "Variable assignment parsing failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::Assignment(stmt)) = result {
        assert_eq!(stmt.variable, "result");
    } else {
        panic!("Expected Assignment statement");
    }
}

#[test]
fn test_parse_let_statement() {
    let mut ctx = create_parser_context("LET $x = 1 + 2");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "LET parse failure: {:?}", result.err());
    if let Ok(Stmt::AssignVariable(stmt)) = result {
        assert_eq!(stmt.name, "x");
        let expr = stmt
            .expression
            .get_expression()
            .expect("expression should resolve");
        assert!(
            matches!(expr, linkrs_core::types::expr::Expression::Binary { .. }),
            "LET RHS should parse as a binary expression, got {:?}",
            expr
        );
    } else {
        panic!("Expected an AssignVariable statement, got {:?}", result);
    }

    let mut ctx = create_parser_context("LET y = 'Alice'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LET without $ parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::AssignVariable(stmt)) = result {
        assert_eq!(stmt.name, "y");
    } else {
        panic!("Expected an AssignVariable statement, got {:?}", result);
    }
}

#[test]
fn test_parse_let_statement_errors() {
    let mut ctx = create_parser_context("LET $x");
    let result = StmtParser::parse_statement(&mut ctx);
    let err = result.expect_err("LET without `=` must fail");
    assert!(
        err.to_string().contains("LET requires an assignment"),
        "unexpected error: {}",
        err
    );

    let mut ctx = create_parser_context("LET $ = 1");
    let result = StmtParser::parse_statement(&mut ctx);
    let err = result.expect_err("LET with empty name must fail");
    assert!(
        err.to_string().contains("Invalid session variable name"),
        "unexpected error: {}",
        err
    );

    let mut ctx = create_parser_context("LET $1x = 1");
    let result = StmtParser::parse_statement(&mut ctx);
    let err = result.expect_err("LET with digit-leading name must fail");
    assert!(
        err.to_string().contains("Invalid session variable name"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_parse_rollback_to_savepoint() {
    let mut ctx = create_parser_context("ROLLBACK TO sp1");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "ROLLBACK TO parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::RollbackTransaction(stmt)) = result {
        assert_eq!(stmt.savepoint_name, Some("sp1".to_string()));
    } else {
        panic!("Expected a RollbackTransaction statement, got {:?}", result);
    }

    let mut ctx = create_parser_context("ROLLBACK");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::RollbackTransaction(stmt)) = result {
        assert_eq!(stmt.savepoint_name, None);
    } else {
        panic!("Expected a RollbackTransaction statement, got {:?}", result);
    }
}

#[test]
fn test_parse_savepoint_and_release() {
    let mut ctx = create_parser_context("SAVEPOINT sp1");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SAVEPOINT parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Savepoint(stmt)) = result {
        assert_eq!(stmt.name, "sp1");
    } else {
        panic!("Expected a Savepoint statement, got {:?}", result);
    }

    let mut ctx = create_parser_context("RELEASE SAVEPOINT sp1");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "RELEASE SAVEPOINT parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::ReleaseSavepoint(stmt)) = result {
        assert_eq!(stmt.name, "sp1");
    } else {
        panic!("Expected a ReleaseSavepoint statement, got {:?}", result);
    }
}

#[test]
fn test_parse_union_statement() {
    let mut ctx = create_parser_context(
        "GO FROM \"player100\" OVER follow UNION GO FROM \"player101\" OVER follow",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "UNION Parse failure: {:?}", result.err());

    if let Ok(Stmt::SetOperation(stmt)) = result {
        assert!(matches!(
            stmt.op_type,
            crate::parser::ast::stmt::SetOperationType::Union
        ));
    } else {
        panic!(
            "Expecting a SetOperation statement, you actually get {:?}",
            result
        );
    }
}

#[test]
fn test_parse_intersect_statement() {
    let mut ctx = create_parser_context(
        "GO FROM \"player100\" OVER follow INTERSECT GO FROM \"player101\" OVER follow",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "INTERSECT parse failure: {:?}",
        result.err()
    );

    if let Ok(Stmt::SetOperation(stmt)) = result {
        assert!(matches!(
            stmt.op_type,
            crate::parser::ast::stmt::SetOperationType::Intersect
        ));
    } else {
        panic!(
            "Expecting a SetOperation statement, you actually get {:?}",
            result
        );
    }
}

#[test]
fn test_parse_minus_statement() {
    let mut ctx = create_parser_context(
        "GO FROM \"player100\" OVER follow MINUS GO FROM \"player101\" OVER follow",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "MINUS parse failure: {:?}", result.err());

    if let Ok(Stmt::SetOperation(stmt)) = result {
        assert!(matches!(
            stmt.op_type,
            crate::parser::ast::stmt::SetOperationType::Minus
        ));
    } else {
        panic!(
            "Expecting a SetOperation statement, you actually get {:?}",
            result
        );
    }
}

#[test]
fn test_parse_comment_on_tag() {
    let mut ctx = create_parser_context("COMMENT ON TAG Person IS 'the person table'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "COMMENT ON TAG parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::CommentOn(stmt)) = result {
        assert_eq!(stmt.comment, "the person table");
    } else {
        panic!("Expected CommentOn statement");
    }
}

#[test]
fn test_parse_comment_on_edge() {
    let mut ctx = create_parser_context("COMMENT ON EDGE Knows IS 'friendship edge'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "COMMENT ON EDGE parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::CommentOn(stmt)) = result {
        assert_eq!(stmt.comment, "friendship edge");
    } else {
        panic!("Expected CommentOn statement");
    }
}

#[test]
fn test_parse_checkpoint() {
    let mut ctx = create_parser_context("CHECKPOINT");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "CHECKPOINT parse failure: {:?}",
        result.err()
    );
    assert!(matches!(result.unwrap(), Stmt::Checkpoint(_)));
}

#[test]
fn test_parse_load_extension() {
    let mut ctx = create_parser_context("LOAD EXTENSION '/tmp/udf_plugin.so'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD EXTENSION parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Extension(stmt)) = result {
        assert_eq!(stmt.action, ExtensionAction::Load);
        assert_eq!(stmt.name, "/tmp/udf_plugin.so");
        assert!(stmt.source.is_none());
    } else {
        panic!("Expected Extension statement");
    }
}

#[test]
fn test_parse_install_extension() {
    let mut ctx = create_parser_context("INSTALL EXTENSION my_udf FROM '/tmp/udf_plugin.so'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "INSTALL EXTENSION parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Extension(stmt)) = result {
        assert_eq!(stmt.action, ExtensionAction::Install);
        assert_eq!(stmt.name, "my_udf");
        assert_eq!(stmt.source.as_deref(), Some("/tmp/udf_plugin.so"));
    } else {
        panic!("Expected Extension statement");
    }
}

#[test]
fn test_parse_uninstall_extension() {
    let mut ctx = create_parser_context("UNINSTALL EXTENSION my_udf");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "UNINSTALL EXTENSION parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Extension(stmt)) = result {
        assert_eq!(stmt.action, ExtensionAction::Uninstall);
        assert_eq!(stmt.name, "my_udf");
        assert!(stmt.source.is_none());
    } else {
        panic!("Expected Extension statement");
    }
}

#[test]
fn test_parse_update_extension() {
    let mut ctx = create_parser_context("UPDATE EXTENSION my_udf");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "UPDATE EXTENSION parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Extension(stmt)) = result {
        assert_eq!(stmt.action, ExtensionAction::Update);
        assert_eq!(stmt.name, "my_udf");
        assert!(stmt.source.is_none());
    } else {
        panic!("Expected Extension statement");
    }
}

#[test]
fn test_parse_show_attached_databases() {
    let mut ctx = create_parser_context("SHOW ATTACHED DATABASES");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW ATTACHED DATABASES parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Show(stmt)) = result {
        assert_eq!(stmt.target, ShowTarget::AttachedDatabases);
    } else {
        panic!("Expected Show statement");
    }
}

#[test]
fn test_parse_show_extensions() {
    let mut ctx = create_parser_context("SHOW EXTENSIONS");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW EXTENSIONS parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Show(stmt)) = result {
        assert_eq!(stmt.target, ShowTarget::Extensions);
    } else {
        panic!("Expected Show statement");
    }
}

#[test]
fn test_parse_load_from() {
    let mut ctx = create_parser_context("LOAD FROM 'data.csv' RETURN *");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD FROM parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::LoadFrom(stmt)) = result {
        assert!(stmt.return_clause.is_some());
    } else {
        panic!("Expected LoadFrom statement");
    }
}

#[test]
fn test_parse_load_from_with_options() {
    let mut ctx =
        create_parser_context("LOAD FROM 'data.csv' (header='true', delimiter=',') RETURN *");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD FROM with options parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_load_from_glob() {
    let mut ctx = create_parser_context("LOAD FROM GLOB('data/*.csv') RETURN *");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD FROM GLOB parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_load_from_table_func() {
    let mut ctx = create_parser_context("LOAD FROM read_csv('data.csv') RETURN *");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD FROM table function parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::LoadFrom(stmt)) = result {
        assert!(matches!(
            stmt.source,
            crate::parser::ast::stmt::ScanSource::TableFunc { .. }
        ));
    } else {
        panic!("Expected LoadFrom statement");
    }
}

#[test]
fn test_parse_load_from_query_source() {
    let mut ctx = create_parser_context("LOAD FROM (MATCH (n) RETURN n) RETURN *");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD FROM query source parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::LoadFrom(stmt)) = result {
        assert!(matches!(
            stmt.source,
            crate::parser::ast::stmt::ScanSource::Query(_)
        ));
    } else {
        panic!("Expected LoadFrom statement");
    }
}

#[test]
fn test_parse_load_with_headers() {
    let mut ctx =
        create_parser_context("LOAD WITH HEADERS (id INT64, name STRING) FROM 'data.csv'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD WITH HEADERS parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::LoadFrom(stmt)) = result {
        assert_eq!(stmt.headers.len(), 2);
        assert_eq!(stmt.headers[0].name, "id");
    } else {
        panic!("Expected LoadFrom statement");
    }
}

#[test]
fn test_parse_load_with_headers_query_source() {
    let mut ctx = create_parser_context(
        "LOAD WITH HEADERS (id, name) FROM (MATCH (n) RETURN n.id AS id, n.name AS name)",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOAD WITH HEADERS query source parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_call_no_args() {
    let mut ctx = create_parser_context("CALL db_version()");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "CALL db_version() parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::InQueryCall(stmt)) = result {
        assert_eq!(stmt.func_name, "db_version");
        assert!(stmt.args.is_empty());
    } else {
        panic!("Expected InQueryCall statement");
    }
}

#[test]
fn test_parse_call_with_args_yield() {
    let mut ctx = create_parser_context("CALL list_touch(1, 2) YIELD result AS x");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "CALL with args and YIELD parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::InQueryCall(stmt)) = result {
        assert_eq!(stmt.func_name, "list_touch");
        assert_eq!(stmt.args.len(), 2);
        assert!(stmt.yield_clause.is_some());
    } else {
        panic!("Expected InQueryCall statement");
    }
}

#[test]
fn test_parse_export_database() {
    let mut ctx = create_parser_context("EXPORT DATABASE '/tmp/db'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "EXPORT DATABASE parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::ExportDatabase(stmt)) = result {
        assert_eq!(stmt.path, "/tmp/db");
    } else {
        panic!("Expected ExportDatabase statement");
    }
}

#[test]
fn test_parse_export_database_with_options() {
    let mut ctx =
        create_parser_context("EXPORT DATABASE '/tmp/db' WITH OPTIONS (format='parquet')");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "EXPORT DATABASE with options parse failure: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_import_database() {
    let mut ctx = create_parser_context("IMPORT DATABASE '/tmp/db'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "IMPORT DATABASE parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::ImportDatabase(stmt)) = result {
        assert_eq!(stmt.path, "/tmp/db");
    } else {
        panic!("Expected ImportDatabase statement");
    }
}

#[test]
fn test_parse_attach_database_with_dbtype() {
    let mut ctx = create_parser_context("ATTACH '/data/kuzu.db' AS mydb (DBTYPE KUZU)");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "ATTACH DATABASE parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::AttachDatabase(stmt)) = result {
        assert_eq!(stmt.path, "/data/kuzu.db");
        assert_eq!(stmt.alias, "mydb");
        assert_eq!(stmt.db_type.as_deref(), Some("KUZU"));
    } else {
        panic!("Expected AttachDatabase statement");
    }
}

#[test]
fn test_parse_attach_database_plain() {
    let mut ctx = create_parser_context("ATTACH '/tmp/csv' AS src");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::AttachDatabase(stmt)) = result {
        assert_eq!(stmt.alias, "src");
        assert_eq!(stmt.db_type, None);
        assert!(stmt.options.is_empty());
    } else {
        panic!("Expected AttachDatabase statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_detach_database() {
    let mut ctx = create_parser_context("DETACH mydb");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::DetachDatabase(stmt)) = result {
        assert_eq!(stmt.alias, "mydb");
    } else {
        panic!("Expected DetachDatabase statement: {:?}", result.err());
    }
}

#[test]
fn test_detach_delete_is_not_detach_database() {
    let mut ctx = create_parser_context("MATCH (n) DETACH DELETE n");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        matches!(result, Ok(Stmt::Match(_))),
        "DETACH DELETE must stay a MATCH statement: {:?}",
        result
    );
}

#[test]
fn test_parse_create_graph_aliases_space() {
    let mut ctx = create_parser_context("CREATE GRAPH mygraph");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Create(s)) = result {
        assert!(
            matches!(s.target, CreateTarget::Space { ref name, .. } if name == "mygraph"),
            "CREATE GRAPH must map to CreateTarget::Space: {:?}",
            s.target
        );
    } else {
        panic!("Expected Create statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_use_graph_aliases_use() {
    let mut ctx = create_parser_context("USE GRAPH mygraph");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Use(s)) = result {
        assert_eq!(s.space, "mygraph");
    } else {
        panic!("Expected Use statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_copy_multi_file_by_column() {
    let mut ctx = create_parser_context("COPY person FROM ('a.csv', 'b.csv') BY COLUMN");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Copy(copy)) = result {
        assert_eq!(copy.file_paths, vec!["a.csv", "b.csv"]);
        assert!(copy.by_column);
    } else {
        panic!("Expected Copy statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_copy_single_file_defaults() {
    let mut ctx = create_parser_context("COPY person FROM 'single.csv'");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Copy(copy)) = result {
        assert_eq!(copy.file_paths, vec!["single.csv"]);
        assert!(!copy.by_column);
    } else {
        panic!("Expected Copy statement: {:?}", result.err());
    }

    let mut ctx = create_parser_context("COPY person FROM ('a.csv', 'b.csv')");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Copy(copy)) = result {
        assert_eq!(copy.file_paths.len(), 2);
        assert!(!copy.by_column);
    } else {
        panic!("Expected Copy statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_copy_to_rejects_multi_file() {
    let mut ctx = create_parser_context("COPY person TO ('a.csv', 'b.csv')");
    assert!(
        StmtParser::parse_statement(&mut ctx).is_err(),
        "COPY TO with multiple files must fail"
    );
}

#[test]
fn test_parse_create_edge_as_query() {
    let mut ctx = create_parser_context("CREATE EDGE worked AS (MATCH (n) RETURN n)");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Create(create)) = result {
        match &create.target {
            CreateTarget::EdgeAsQuery { name, query_text } => {
                assert_eq!(name, "worked");
                assert!(query_text.contains("MATCH"));
            }
            other => panic!("Expected EdgeAsQuery: {other:?}"),
        }
    } else {
        panic!("Expected Create statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_create_tag_as_query() {
    let mut ctx = create_parser_context("CREATE TAG adults AS (MATCH (n) RETURN n)");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Create(create)) = result {
        assert!(
            matches!(create.target, CreateTarget::TagAsQuery { ref name, .. } if name == "adults"),
            "expected TagAsQuery: {:?}",
            create.target
        );
    } else {
        panic!("Expected Create statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_alter_edge_add_drop_from() {
    let mut ctx = create_parser_context("ALTER EDGE works_at ADD FROM person TO company");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Alter(alter)) = result {
        match &alter.target {
            AlterTarget::AddFrom {
                edge_name,
                src_tag,
                dst_tag,
            } => {
                assert_eq!(edge_name, "works_at");
                assert_eq!(src_tag, "person");
                assert_eq!(dst_tag, "company");
            }
            other => panic!("Expected AddFrom: {other:?}"),
        }
    } else {
        panic!("Expected Alter statement: {:?}", result.err());
    }

    let mut ctx = create_parser_context("ALTER EDGE works_at DROP FROM person TO company");
    let result = StmtParser::parse_statement(&mut ctx);
    if let Ok(Stmt::Alter(alter)) = result {
        assert!(
            matches!(alter.target, AlterTarget::DropFrom { ref edge_name, .. } if edge_name == "works_at"),
            "expected DropFrom: {:?}",
            alter.target
        );
    } else {
        panic!("Expected Alter statement: {:?}", result.err());
    }
}

#[test]
fn test_parse_show_indexes_stats_and_index() {
    let mut ctx = create_parser_context("SHOW INDEXES");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW INDEXES parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Show(stmt)) = result {
        assert_eq!(stmt.target, ShowTarget::Indexes);
    } else {
        panic!("Expected Show statement");
    }

    let mut ctx = create_parser_context("SHOW STATS");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW STATS parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Show(stmt)) = result {
        assert_eq!(stmt.target, ShowTarget::Stats);
    } else {
        panic!("Expected Show statement");
    }

    let mut ctx = create_parser_context("SHOW INDEX person_name_index");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW INDEX parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Show(stmt)) = result {
        assert_eq!(
            stmt.target,
            ShowTarget::Index("person_name_index".to_string())
        );
    } else {
        panic!("Expected Show statement");
    }
}

#[test]
fn test_parse_fulltext_match_lookup_show_describe_dispatch() {
    let mut ctx = create_parser_context("MATCH 'docs' WHERE FULLTEXT_MATCH(content, 'database')");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "MATCH FULLTEXT parse failure: {:?}",
        result.err()
    );
    assert!(
        matches!(result.unwrap(), crate::parser::ast::Stmt::MatchFulltext(_)),
        "expected MatchFulltext statement"
    );

    let mut ctx = create_parser_context("LOOKUP ON article INDEX idx_article WHERE 'database'");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "LOOKUP FULLTEXT parse failure: {:?}",
        result.err()
    );
    assert!(
        matches!(result.unwrap(), crate::parser::ast::Stmt::LookupFulltext(_)),
        "expected LookupFulltext statement"
    );

    let mut ctx = create_parser_context("SHOW FULLTEXT INDEX");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "SHOW FULLTEXT INDEX parse failure: {:?}",
        result.err()
    );
    assert!(
        matches!(
            result.unwrap(),
            crate::parser::ast::Stmt::ShowFulltextIndex(_)
        ),
        "expected ShowFulltextIndex statement"
    );

    let mut ctx = create_parser_context("DESCRIBE FULLTEXT INDEX idx_article");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "DESCRIBE FULLTEXT INDEX parse failure: {:?}",
        result.err()
    );
    assert!(
        matches!(
            result.unwrap(),
            crate::parser::ast::Stmt::DescribeFulltextIndex(_)
        ),
        "expected DescribeFulltextIndex statement"
    );
}

#[test]
fn test_parse_plain_lookup_still_parses_after_fulltext_routing() {
    let mut ctx = create_parser_context("LOOKUP ON person WHERE person.age > 20 YIELD person.name");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "plain LOOKUP parse failure: {:?}",
        result.err()
    );
    assert!(
        matches!(result.unwrap(), Stmt::Lookup(_)),
        "expected Lookup statement"
    );
}

#[test]
fn test_parse_merge_bare_set_applies_to_both_branches() {
    let mut ctx = create_parser_context("MERGE (n:person {name: 'a'}) SET n.age = 30");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "MERGE bare SET parse failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Merge(merge)) = result {
        let on_create = merge.on_create.expect("bare SET must populate ON CREATE");
        let on_match = merge.on_match.expect("bare SET must populate ON MATCH");
        assert_eq!(on_create.assignments.len(), 1);
        assert_eq!(on_match.assignments.len(), 1);
        assert_eq!(on_create.assignments[0].property, "age");
        assert_eq!(on_match.assignments[0].property, "age");
    } else {
        panic!("Expected Merge statement");
    }
}

#[test]
fn test_parse_merge_bare_set_merges_with_on_match_clause() {
    let mut ctx = create_parser_context("MERGE (n:person) ON MATCH SET n.a = 1 SET n.b = 2");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "MERGE SET merge failure: {:?}",
        result.err()
    );
    if let Ok(Stmt::Merge(merge)) = result {
        let on_match = merge.on_match.expect("ON MATCH must be present");
        let on_create = merge.on_create.expect("bare SET must populate ON CREATE");
        assert_eq!(on_match.assignments.len(), 2);
        assert_eq!(on_create.assignments.len(), 1);
        assert_eq!(on_create.assignments[0].property, "b");
    } else {
        panic!("Expected Merge statement");
    }
}

#[test]
fn test_parse_named_path_binding() {
    let mut ctx = create_parser_context("MATCH p = (a:person)-[e:knows]->(b:person) RETURN a");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "named path parse failure: {:?}",
        result.err()
    );
    if let Ok(crate::parser::ast::Stmt::Match(m)) = result {
        assert_eq!(m.patterns.len(), 1);
        if let crate::parser::ast::pattern::Pattern::Path(path) = &m.patterns[0] {
            assert_eq!(path.name.as_deref(), Some("p"));
            assert_eq!(path.elements.len(), 3);
        } else {
            panic!("expected path pattern");
        }
    } else {
        panic!("Expected Match statement");
    }

    let mut ctx = create_parser_context("MATCH p = (a) RETURN a");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "single-node named path parse failure: {:?}",
        result.err()
    );
    if let Ok(crate::parser::ast::Stmt::Match(m)) = result {
        if let crate::parser::ast::pattern::Pattern::Path(path) = &m.patterns[0] {
            assert_eq!(path.name.as_deref(), Some("p"));
            assert_eq!(path.elements.len(), 1);
        } else {
            panic!("expected path pattern");
        }
    } else {
        panic!("Expected Match statement");
    }
}

#[test]
fn test_parse_named_path_binding_rejects_bare_variable() {
    let mut ctx = create_parser_context("MATCH p = q RETURN p");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_err() || ctx.has_errors(),
        "binding a bare variable as a path must fail"
    );
}

#[test]
fn test_parse_match_where_inline_pattern_predicate() {
    let mut ctx = create_parser_context("MATCH (a:person) WHERE (a)-[:knows]->(b:person) RETURN a");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "inline pattern predicate parse failure: {:?}",
        result.err()
    );
    if let Ok(crate::parser::ast::Stmt::Match(m)) = result {
        let where_clause = m.where_clause.expect("WHERE must be present");
        let expr = where_clause
            .expression()
            .expect("WHERE must be registered")
            .inner()
            .clone();
        match expr {
            linkrs_core::Expression::Exists { body } => {
                assert_eq!(body.patterns.len(), 1);
                assert!(
                    body.patterns[0].contains("knows"),
                    "unexpected pattern text: {}",
                    body.patterns[0]
                );
            }
            other => panic!("expected EXISTS rewrite, got {:?}", other),
        }
    } else {
        panic!("Expected Match statement");
    }

    // Combined with an ordinary conjunct the rewrite still applies.
    let mut ctx = create_parser_context(
        "MATCH (a:person) WHERE a.age > 20 AND (a)-[:knows]->(b:person) RETURN a",
    );
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "conjunctive inline pattern parse failure: {:?}",
        result.err()
    );

    // A lone parenthesized variable keeps its expression meaning.
    let mut ctx = create_parser_context("MATCH (a) WHERE (a) RETURN a");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(
        result.is_ok(),
        "parenthesized variable must still parse: {:?}",
        result.err()
    );
}

#[test]
fn test_parse_return_sample_clause() {
    let mut ctx = create_parser_context("MATCH (n) RETURN n SAMPLE 3");
    let result = StmtParser::parse_statement(&mut ctx);
    assert!(result.is_ok(), "SAMPLE parse failure: {:?}", result.err());
    if let Ok(crate::parser::ast::Stmt::Match(m)) = result {
        let return_clause = m.return_clause.expect("RETURN must be present");
        let sample = return_clause.sample.expect("SAMPLE must be parsed");
        assert_eq!(sample.count, 3);
    } else {
        panic!("Expected Match statement");
    }
}

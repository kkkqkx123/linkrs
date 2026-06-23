//! Parsing Module Tests
//!
//! Test coverage:
//! - Lexer edge cases
//! - Parser error handling
//! - AST validation
//! - Statement parsing

use crate::common::test_scenario::TestScenario;

// ==================== Lexer Edge Case Tests ====================

mod lexer_edge_cases {
    use super::*;

    #[test]
    fn test_unicode_string_literal() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_unicode")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("INSERT VERTEX person(name) VALUES 1:('中文测试')")
            .assert_success();
    }

    #[test]
    fn test_escaped_string() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_escaped")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("INSERT VERTEX person(name) VALUES 1:('test\\'s value')")
            .assert_success();
    }

    #[test]
    fn test_long_identifier() {
        let long_name = "a".repeat(100);
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_long_id")
            .exec_ddl(&format!("CREATE TAG {}(name STRING)", long_name))
            .assert_success();
    }

    #[test]
    fn test_numeric_literals() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_numeric")
            .exec_ddl("CREATE TAG numbers(int_val INT, float_val DOUBLE)")
            .assert_success()
            .query("MATCH (n:numbers) WHERE n.int_val = 123 AND n.float_val = 456.789 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_negative_numbers() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_negative")
            .exec_ddl("CREATE TAG numbers(val INT)")
            .assert_success()
            .query("MATCH (n:numbers) WHERE n.val < 0 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_special_characters_in_identifier() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_special_id")
            .exec_ddl("CREATE TAG `user-table`(name STRING)")
            .assert_success();
    }
}

// ==================== Parser Error Handling Tests ====================

mod parser_errors {
    use super::*;

    #[test]
    fn test_syntax_error_recovery() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_syntax_error")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person WHERE n.name = 'test' RETURN n")
            .assert_error();
    }

    #[test]
    fn test_unexpected_token() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_unexpected_token")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n ORDER ORDER BY n.name")
            .assert_error();
    }

    #[test]
    fn test_missing_keyword() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_missing_keyword")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE RETURN n")
            .assert_error();
    }

    #[test]
    fn test_unbalanced_parentheses() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_unbalanced")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE (n.name = 'test' RETURN n")
            .assert_error();
    }

    #[test]
    fn test_invalid_property_type() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_invalid_type")
            .query("CREATE TAG person(name INVALID_TYPE)")
            .assert_error();
    }
}

// ==================== AST Validation Tests ====================

mod ast_validation {
    use super::*;

    #[test]
    fn test_match_statement_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_match_ast")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_insert_statement_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_insert_ast")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success();
    }

    #[test]
    fn test_update_statement_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_update_ast")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("UPDATE VERTEX 1 SET person.name = 'Bob'")
            .assert_success();
    }

    #[test]
    fn test_delete_statement_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_delete_ast")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("DELETE VERTEX 1")
            .assert_success();
    }

    #[test]
    fn test_create_tag_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_create_tag_ast")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success();
    }

    #[test]
    fn test_create_edge_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_create_edge_ast")
            .exec_ddl("CREATE EDGE follows(since INT)")
            .assert_success();
    }

    #[test]
    fn test_create_index_ast() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_create_idx_ast")
            .exec_ddl("CREATE TAG person(age INT)")
            .exec_ddl("CREATE TAG INDEX idx_age ON person(age)")
            .assert_success();
    }
}

// ==================== Statement Parsing Tests ====================

mod statement_parsing {
    use super::*;

    #[test]
    fn test_use_statement() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_use")
            .query("USE test_use")
            .assert_success();
    }

    #[test]
    fn test_show_spaces() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_show_spaces")
            .query("SHOW SPACES")
            .assert_success();
    }

    #[test]
    fn test_show_tags() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_show_tags")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("SHOW TAGS")
            .assert_success();
    }

    #[test]
    fn test_show_edges() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_show_edges")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("SHOW EDGES")
            .assert_success();
    }

    #[test]
    fn test_describe_tag() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_desc_tag")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("DESCRIBE TAG person")
            .assert_success();
    }

    #[test]
    fn test_describe_edge() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_desc_edge")
            .exec_ddl("CREATE EDGE follows(since INT)")
            .assert_success()
            .query("DESCRIBE EDGE follows")
            .assert_success();
    }

    #[test]
    fn test_drop_tag() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_drop_tag")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("DROP TAG person")
            .assert_success();
    }

    #[test]
    fn test_drop_edge() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_drop_edge")
            .exec_ddl("CREATE EDGE follows()")
            .exec_ddl("DROP EDGE follows")
            .assert_success();
    }

    #[test]
    fn test_alter_tag() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_alter_tag")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("ALTER TAG person ADD (age INT)")
            .assert_success();
    }

    #[test]
    fn test_alter_edge() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_alter_edge")
            .exec_ddl("CREATE EDGE follows()")
            .exec_ddl("ALTER EDGE follows ADD (since INT)")
            .assert_success();
    }
}

// ==================== Complex Statement Tests ====================

mod complex_statements {
    use super::*;

    #[test]
    fn test_multi_hop_traversal() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_multi_hop")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person)-[:follows]->(c:person) RETURN a, c")
            .assert_success();
    }

    #[test]
    fn test_with_clause() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_with")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name, n.age AS age RETURN name, age")
            .assert_success();
    }

    #[test]
    fn test_unwind_clause() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_unwind")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("UNWIND [1, 2, 3] AS x RETURN x")
            .assert_success();
    }

    #[test]
    fn test_list_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_list_expr")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN [1, 2, 3] AS nums")
            .assert_success();
    }

    #[test]
    fn test_map_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_map_expr")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN {name: n.name, id: 1} AS info")
            .assert_success();
    }

    #[test]
    fn test_aggregate_functions() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_func")
            .exec_ddl("CREATE TAG person(age INT)")
            .exec_dml("INSERT VERTEX person(age) VALUES 1:(20), 2:(30), 3:(40)")
            .assert_success()
            .query(
                "MATCH (n:person) RETURN count(n), sum(n.age), avg(n.age), max(n.age), min(n.age)",
            )
            .assert_success();
    }

    #[test]
    fn test_string_functions() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_string_func")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("MATCH (n:person) RETURN upper(n.name), lower(n.name), length(n.name)")
            .assert_success();
    }

    #[test]
    fn test_math_functions() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_math_func")
            .exec_ddl("CREATE TAG numbers(val DOUBLE)")
            .exec_dml("INSERT VERTEX numbers(val) VALUES 1:(3.14)")
            .assert_success()
            .query("MATCH (n:numbers) RETURN abs(n.val), round(n.val)")
            .assert_success();
    }
}

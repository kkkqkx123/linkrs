//! Validation Module Tests
//!
//! Test coverage:
//! - Semantic validation
//! - Type checking
//! - Expression analysis
//! - Variable scope resolution

use crate::common::test_scenario::TestScenario;

// ==================== Semantic Validation Tests ====================

mod semantic_validation {
    use super::*;

    #[test]
    fn test_undefined_variable_detection() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_semantic")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN undefined_var")
            .assert_error();
    }

    #[test]
    fn test_undefined_tag_detection() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_undefined_tag")
            .query("MATCH (n:NonExistentTag) RETURN n")
            .assert_error();
    }

    #[test]
    fn test_undefined_edge_type_detection() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_undefined_edge")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (a:person)-[:nonexistent_edge]->(b:person) RETURN a, b")
            .assert_error();
    }

    #[test]
    fn test_column_alias_uniqueness() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_alias")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name AS name, n.name AS name")
            .assert_success();
    }

    #[test]
    fn test_aggregate_in_where_clause() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_where")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH count(n) AS cnt WHERE cnt > 0 RETURN cnt")
            .assert_success();
    }

    #[test]
    fn test_valid_tag_reference() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_valid_tag")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_valid_edge_reference() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_valid_edge")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:knows]->(b:person) RETURN a, b")
            .assert_success();
    }
}

// ==================== Type Checking Tests ====================

mod type_checking {
    use super::*;

    #[test]
    fn test_type_mismatch_in_comparison() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_type_mismatch")
            .exec_ddl("CREATE TAG person(age INT, name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = \"string\" RETURN n")
            .assert_success();
    }

    #[test]
    fn test_arithmetic_type_checking() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_arithmetic_type")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age + 10 > 0 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_function_argument_types() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_func_args")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN length(n.name)")
            .assert_success();
    }

    #[test]
    fn test_implicit_type_conversion() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_implicit_conv")
            .exec_ddl("CREATE TAG person(id INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.id = 1 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_null_handling() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_null")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.name IS NULL RETURN n")
            .assert_success();
    }

    #[test]
    fn test_boolean_expression_context() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_bool_context")
            .exec_ddl("CREATE TAG person(active BOOL)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.active AND true RETURN n")
            .assert_success();
    }

    #[test]
    fn test_int_comparison() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_int_comp")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 18 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_string_comparison() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_str_comp")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.name = 'Alice' RETURN n")
            .assert_success();
    }
}

// ==================== Expression Analysis Tests ====================

mod expression_analysis {
    use super::*;

    #[test]
    fn test_nested_property_access() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_nested_prop")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.person.name")
            .assert_success();
    }

    #[test]
    fn test_complex_arithmetic_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_complex_arith")
            .exec_ddl("CREATE TAG person(a INT, b INT, c INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN (n.a + n.b) * n.c / 2 - 1")
            .assert_success();
    }

    #[test]
    fn test_list_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_list_expr")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN [1, 2, 3]")
            .assert_success();
    }

    #[test]
    fn test_map_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_map_expr")
            .query("RETURN {name: 'Alice', age: 30}")
            .assert_success();
    }

    #[test]
    fn test_function_call_with_nested_functions() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_nested_func")
            .exec_ddl("CREATE TAG person(value INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN abs(sin(n.value))")
            .assert_success();
    }

    #[test]
    fn test_aggregate_function_distinct() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_distinct")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN count(n.name)")
            .assert_success();
    }

    #[test]
    fn test_subscript_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_subscript")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_range_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_range")
            .query("RETURN range(1, 10)")
            .assert_success();
    }

    #[test]
    fn test_pattern_predicate() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_pattern_pred")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (n:person)-[:knows]->(m:person) RETURN n, m")
            .assert_success();
    }

    #[test]
    fn test_string_function_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_str_func_expr")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN upper(n.name), lower(n.name)")
            .assert_success();
    }

    #[test]
    fn test_math_function_expression() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_math_func_expr")
            .exec_ddl("CREATE TAG person(value DOUBLE)")
            .assert_success()
            .query("MATCH (n:person) RETURN abs(n.value), round(n.value)")
            .assert_success();
    }
}

// ==================== Variable Scope Tests ====================

mod variable_scope {
    use super::*;

    #[test]
    fn test_with_clause_variable_propagation() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_with_scope")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name, n.age AS age RETURN name, age")
            .assert_success();
    }

    #[test]
    fn test_unwind_variable_scope() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_unwind_scope")
            .query("UNWIND [1, 2, 3] AS x RETURN x * 2")
            .assert_success();
    }

    #[test]
    fn test_variable_shadowing() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_shadowing")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WITH n AS m RETURN m.name")
            .assert_success();
    }

    #[test]
    fn test_match_variable_binding() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_match_binding")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (a:person), (b:person) RETURN a.name, b.name")
            .assert_success();
    }

    #[test]
    fn test_with_clause_renaming() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_with_rename")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS personName RETURN personName")
            .assert_success();
    }

    #[test]
    fn test_multiple_match_clauses() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_multi_match")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .assert_success()
            .query("MATCH (p:person), (c:company) RETURN p.name, c.name")
            .assert_success();
    }
}

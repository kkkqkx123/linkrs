//! Extended Management and Auxiliary Statement Integration Tests
//!
//! This file demonstrates how to use the new test framework to validate
//! actual execution effects of management statements.
//!
//! Test coverage:
//! - USE - Graph space switching with validation
//! - SHOW - Information display with result validation
//! - EXPLAIN - Query plan analysis with format options
//! - PROFILE - Performance analysis
//! - GROUP BY - Grouping operations
//! - RETURN - Return statement with data
//! - WITH - Intermediate result handling
//! - UNWIND - List expansion
//! - PIPE - Pipeline operations with data flow
//! - Variable Assignment
//! - Set Operations (UNION, INTERSECT, MINUS)

mod common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;

// ==================== USE Statement Extended Tests ====================

#[test]
fn test_use_space_and_verify() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("space_a")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        // Switch to a different space context
        .exec_ddl("USE space_a")
        .assert_success()
        .query("FETCH PROP ON Person 1")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_use_nonexistent_space() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("USE nonexistent_space_xyz")
        .assert_error();
}

// ==================== SHOW Statement Extended Tests ====================

#[test]
fn test_show_spaces_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space_1")
        .setup_space("test_space_2")
        .query("SHOW SPACES")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_show_tags_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE TAG Company(name STRING)")
        .assert_success()
        .query("SHOW TAGS")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_show_edges_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_ddl("CREATE EDGE WORKS_AT(since DATE)")
        .assert_success()
        .query("SHOW EDGES")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_show_hosts() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW HOSTS")
        .assert_success();
}

#[test]
fn test_show_parts() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW PARTS")
        .assert_success();
}

#[test]
fn test_show_sessions() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW SESSIONS")
        .assert_success();
}

#[test]
fn test_show_queries() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW QUERIES")
        .assert_success();
}

#[test]
fn test_show_configs() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW CONFIGS")
        .assert_success();
}

// ==================== EXPLAIN Statement Extended Tests ====================

#[test]
fn test_explain_match_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("EXPLAIN MATCH (n:Person) RETURN n")
        .assert_success();
}

#[test]
fn test_explain_go_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("EXPLAIN GO FROM 1 OVER KNOWS")
        .assert_success();
}

#[test]
fn test_explain_format_table() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .query("EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n")
        .assert_success();
}

#[test]
fn test_explain_format_dot() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .query("EXPLAIN FORMAT = DOT MATCH (n:Person) RETURN n")
        .assert_success();
}

// ==================== PROFILE Statement Extended Tests ====================

#[test]
fn test_profile_match() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .query("PROFILE MATCH (n:Person) RETURN n")
        .assert_success();
}

#[test]
fn test_profile_go() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("PROFILE GO FROM 1 OVER KNOWS")
        .assert_success();
}

#[test]
fn test_profile_with_limit() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .query("PROFILE MATCH (n:Person) RETURN n LIMIT 2")
        .assert_success();
}

// ==================== GROUP BY Statement Extended Tests ====================

#[test]
fn test_group_by_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT, city STRING)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age, city) VALUES
                1:('Alice', 30, 'NYC'),
                2:('Bob', 25, 'LA'),
                3:('Charlie', 35, 'NYC'),
                4:('David', 28, 'LA')
        "#,
        )
        .assert_success()
        .query("GROUP BY city YIELD city, count(*) AS count")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_group_by_with_aggregation() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Product(name STRING, category STRING, price DOUBLE)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Product(name, category, price) VALUES
                1:('Laptop', 'Electronics', 999.99),
                2:('Mouse', 'Electronics', 29.99),
                3:('Desk', 'Furniture', 299.99),
                4:('Chair', 'Furniture', 199.99)
        "#,
        )
        .assert_success()
        .query("GROUP BY category YIELD category, avg(price) AS avg_price, count(*) AS count")
        .assert_success()
        .assert_result_count(2);
}

// ==================== RETURN Statement Extended Tests ====================

#[test]
fn test_return_literal_values() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN 'Hello World'")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::String("Hello World".into())]);
}

#[test]
fn test_return_expression() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN 1 + 2 AS result")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_return_list() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN [1, 2, 3] AS numbers")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_return_map() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN {name: 'Alice', age: 30} AS person")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_return_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("MATCH (n:Person) RETURN n.name, n.age")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::String("Alice".into()), Value::Int(30)]);
}

#[test]
fn test_return_distinct() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, city STRING)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, city) VALUES
                1:('Alice', 'NYC'),
                2:('Bob', 'NYC'),
                3:('Charlie', 'LA')
        "#,
        )
        .assert_success()
        .query("MATCH (n:Person) RETURN DISTINCT n.city")
        .assert_success()
        .assert_result_count(2);
}

// ==================== WITH Statement Extended Tests ====================

#[test]
fn test_with_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("WITH 1 AS x RETURN x")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_with_expression() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("WITH 2 + 3 AS sum RETURN sum")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_with_chain() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("WITH 1 AS a WITH a + 1 AS b RETURN b")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_with_match() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age) VALUES
                1:('Alice', 30),
                2:('Bob', 25),
                3:('Charlie', 35)
        "#,
        )
        .assert_success()
        .query(
            r#"
            MATCH (n:Person)
            WITH n.name AS name, n.age AS age
            WHERE age > 25
            RETURN name, age
        "#,
        )
        .assert_success()
        .assert_result_count(2);
}

// ==================== UNWIND Statement Extended Tests ====================

#[test]
fn test_unwind_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("UNWIND [1, 2, 3] AS n RETURN n")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_unwind_string_list() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("UNWIND ['a', 'b', 'c'] AS s RETURN s")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_unwind_with_expression() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("UNWIND [1, 2, 3] AS n RETURN n * 2 AS doubled")
        .assert_success()
        .assert_result_count(3);
}

#[test]
fn test_unwind_with_match() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name) VALUES
                1:('Alice'),
                2:('Bob')
        "#,
        )
        .assert_success()
        .query(
            r#"
            MATCH (n:Person)
            WITH n.name AS names
            UNWIND names AS name
            RETURN name
        "#,
        )
        .assert_success();
}

// ==================== PIPE Statement Extended Tests ====================

#[test]
fn test_pipe_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS | YIELD target.name")
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_pipe_multiple_stages() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age) VALUES
                1:('Alice', 30),
                2:('Bob', 25),
                3:('Charlie', 35)
        "#,
        )
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .query(
            r#"
            GO FROM 1 OVER KNOWS
            | YIELD target.name AS name, target.age AS age
            | WHERE age > 25
            | RETURN name
        "#,
        )
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_pipe_with_fetch() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS | YIELD target.id AS id | FETCH PROP ON Person $-.id")
        .assert_success();
}

// ==================== Variable Assignment Extended Tests ====================

#[test]
fn test_variable_assignment() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query("$result = GO FROM 1 OVER KNOWS")
        .assert_success();
}

// ==================== Set Operations Extended Tests ====================

#[test]
fn test_union_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 1 -> 3:('2021-01-01')")
        .assert_success()
        .query(
            r#"
            GO FROM 1 OVER KNOWS YIELD target.name AS name
            UNION
            GO FROM 1 OVER FOLLOWS YIELD target.name AS name
        "#,
        )
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_intersect_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query(
            r#"
            GO FROM 1 OVER KNOWS YIELD target.name AS name
            INTERSECT
            GO FROM 1 OVER FOLLOWS YIELD target.name AS name
        "#,
        )
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_minus_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 1 -> 2:('2020-01-01')")
        .assert_success()
        .query(
            r#"
            GO FROM 1 OVER KNOWS YIELD target.name AS name
            MINUS
            GO FROM 1 OVER FOLLOWS YIELD target.name AS name
        "#,
        )
        .assert_success()
        .assert_result_count(1);
}

// ==================== Complex Workflow Tests ====================

#[test]
fn test_management_workflow_complete() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        // Setup
        .setup_space("management_test")
        .exec_ddl("CREATE TAG Person(name STRING, age INT, city STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .assert_success()
        // Insert data
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age, city) VALUES
                1:('Alice', 30, 'NYC'),
                2:('Bob', 25, 'LA'),
                3:('Charlie', 35, 'NYC'),
                4:('David', 28, 'LA')
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT EDGE KNOWS(since, strength) VALUES
                1 -> 2:('2020-01-01', 0.9),
                1 -> 3:('2021-01-01', 0.8),
                2 -> 4:('2022-01-01', 0.7)
        "#,
        )
        .assert_success()
        // Show operations
        .query("SHOW TAGS")
        .assert_success()
        .assert_result_count(1)
        .query("SHOW EDGES")
        .assert_success()
        .assert_result_count(1)
        // Explain query
        .query("EXPLAIN MATCH (n:Person) RETURN n")
        .assert_success()
        // Profile query
        .query("PROFILE GO FROM 1 OVER KNOWS")
        .assert_success()
        // Complex pipeline
        .query(
            r#"
            GO FROM 1 OVER KNOWS
            | YIELD target.name AS name, target.age AS age, target.city AS city
            | WHERE age > 25
            | RETURN name, city
        "#,
        )
        .assert_success()
        // Group by
        .query(
            r#"
            MATCH (n:Person)
            WITH n.city AS city, n.age AS age
            GROUP BY city YIELD city, avg(age) AS avg_age
        "#,
        )
        .assert_success()
        .assert_result_count(2);
}

#[test]
fn test_pipe_with_unwind() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .query(
            r#"
            GO FROM 1 OVER KNOWS
            | YIELD target.name AS name
            | COLLECT LIST(name) AS names
            | UNWIND names AS n
            | RETURN n
        "#,
        )
        .assert_success();
}

#[test]
fn test_return_with_aggregation() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age) VALUES
                1:('Alice', 30),
                2:('Bob', 25),
                3:('Charlie', 35)
        "#,
        )
        .assert_success()
        .query("MATCH (n:Person) RETURN count(*) AS total, avg(n.age) AS avg_age")
        .assert_success()
        .assert_result_count(1);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_invalid_use_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("USE")
        .assert_error();
}

#[test]
fn test_invalid_show_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("SHOW INVALID")
        .assert_error();
}

#[test]
fn test_invalid_explain_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("EXPLAIN")
        .assert_error();
}

#[test]
fn test_invalid_return_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("RETURN")
        .assert_error();
}

#[test]
fn test_invalid_with_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("WITH")
        .assert_error();
}

#[test]
fn test_invalid_unwind_statement() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .query("UNWIND")
        .assert_error();
}

//! DDL Constraints Tests
//!
//! Test coverage:
//! - DEFAULT value constraints
//! - NOT NULL constraints
//! - NULL constraints

use super::common;

use common::test_scenario::TestScenario;
use common::TestStorage;
use graphdb_query::core::stats::StatsManager;
use graphdb_query::core::Value;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::parser::Parser;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::collections::HashMap;
use std::sync::Arc;

// ==================== DEFAULT Value Tests ====================

#[test]
fn test_create_tag_with_default_value() {
    let query = "CREATE TAG Person(name: STRING, age: INT DEFAULT 18)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with DEFAULT parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_with_default_string() {
    let query = "CREATE TAG Person(name: STRING DEFAULT 'unknown', email: STRING DEFAULT 'test@example.com')";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with string DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_tag_with_default_null() {
    let query = "CREATE TAG Person(name: STRING, nickname: STRING DEFAULT NULL)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with NULL DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_tag_with_default_int() {
    let query =
        "CREATE TAG Product(name: STRING, quantity: INT DEFAULT 0, price: DOUBLE DEFAULT 0.0)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with numeric DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_tag_with_default_bool() {
    let query =
        "CREATE TAG User(name: STRING, active: BOOL DEFAULT true, verified: BOOL DEFAULT false)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with bool DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== NOT NULL Constraint Tests ====================

#[test]
fn test_create_tag_with_not_null() {
    let query = "CREATE TAG Person(name: STRING NOT NULL, age: INT NOT NULL)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with NOT NULL parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_with_nullable() {
    let query = "CREATE TAG Person(name: STRING NOT NULL, nickname: STRING NULL)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with NULL constraint parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_tag_with_not_null_and_default() {
    let query = "CREATE TAG Person(name: STRING NOT NULL, age: INT NOT NULL DEFAULT 0)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with NOT NULL and DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Mixed Constraints Tests ====================

#[test]
fn test_create_tag_with_mixed_constraints() {
    let query = r#"CREATE TAG User(
        id: INT NOT NULL,
        name: STRING NOT NULL DEFAULT 'unknown',
        email: STRING,
        age: INT DEFAULT 0,
        active: BOOL DEFAULT true,
        created_at: TIMESTAMP DEFAULT "2024-01-01 00:00:00"
    )"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with mixed constraints parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_edge_with_default() {
    let query = "CREATE EDGE KNOWS(since: DATE DEFAULT '2024-01-01', strength: DOUBLE DEFAULT 1.0)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE with DEFAULT parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_edge_with_not_null() {
    let query = "CREATE EDGE KNOWS(since: DATE NOT NULL, note: STRING)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE EDGE with NOT NULL parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Error Handling Tests ====================

#[test]
fn test_ddl_error_handling() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let invalid_queries = vec![
        "CREATE TAG Person",
        "ALTER TAG Person ADD",
        "DROP TAG",
        "DESCRIBE",
    ];

    for query in invalid_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_err(),
            "Invalid query should return error: {}",
            query
        );
    }
}

// ==================== DEFAULT Value Execution Tests ====================

#[test]
fn test_default_value_execution_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT DEFAULT 18)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(18)),
            ]),
        );
}

#[test]
fn test_default_value_execution_override() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT DEFAULT 18)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 25)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(25)),
            ]),
        );
}

#[test]
fn test_default_value_string_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING DEFAULT 'unknown')")
        .assert_success()
        // Insert with name value, relying on DEFAULT for other properties
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Bob')")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([("name", Value::String("Bob".into()))]),
        );
}

// ==================== NOT NULL Execution Tests ====================

#[test]
fn test_not_null_constraint_insert_with_value() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING NOT NULL, age: INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

#[test]
fn test_not_null_constraint_reject_null() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING NOT NULL, age: INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(age) VALUES 1:(30)")
        .assert_error();
}

#[test]
fn test_default_with_not_null_constraint() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING NOT NULL DEFAULT 'unknown', age: INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(age) VALUES 1:(30)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("unknown".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

#[test]
fn test_edge_default_value_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl(
            "CREATE EDGE KNOWS(since: DATE DEFAULT '2024-01-01', strength: DOUBLE DEFAULT 1.0)",
        )
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(strength) VALUES 1 -> 2:(0.5)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

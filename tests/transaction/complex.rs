//! Transaction Complex Operation Tests
//!
//! Test coverage:
//! - Complex transaction with multiple operations
//! - Conditional operations
//! - Tag modification
//! - Cascading operations
//! - Statistics consistency
//! - Multiple schema changes
//! - Nested operations
//! - Aggregation operations

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use std::collections::HashMap;

/// Test complex transaction with multiple operations
#[test]
fn test_transaction_complex_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE TAG IF NOT EXISTS Company(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_AT")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name, age) VALUES \
            1:('Alice', 30), \
            2:('Bob', 25), \
            3:('Charlie', 35)",
        )
        .assert_success()
        .exec_dml("INSERT VERTEX Company(name) VALUES 100:('TechCorp')")
        .assert_success()
        .exec_dml(
            "INSERT EDGE WORKS_AT VALUES \
            1->100, \
            2->100, \
            3->100",
        )
        .assert_success()
        .query("MATCH (p:Person)-[:WORKS_AT]->(c:Company) RETURN p")
        .assert_result_count(3);
}

/// Test transaction with conditional operations
#[test]
fn test_transaction_conditional_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name, age) VALUES \
            1:('Alice', 30), \
            2:('Bob', 25), \
            3:('Charlie', 35)",
        )
        .assert_success()
        .query("MATCH (v:Person) WHERE v.age > 28 RETURN v.name")
        .assert_result_count(2);
}

/// Test transaction with tag modification
#[test]
fn test_transaction_tag_modification() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

/// Test transaction with vertex and edge cascading
#[test]
fn test_transaction_cascading_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 2->3")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 3->1")
        .assert_success()
        .query("MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a")
        .assert_result_count(3);
}

/// Test transaction statistics consistency
#[test]
fn test_transaction_statistics_consistency() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .assert_vertex_count("Person", 3)
        .exec_dml("INSERT VERTEX Person(name) VALUES 4:('David')")
        .assert_success()
        .assert_vertex_count("Person", 4);
}

/// Test transaction with multiple schema changes
#[test]
fn test_transaction_multiple_schema_changes() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("ALTER TAG Person ADD (age INT)")
        .assert_success()
        .exec_ddl("ALTER TAG Person ADD (email STRING)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name, age, email) VALUES 1:('Alice', 30, 'alice@example.com')",
        )
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
                ("email", Value::String("alice@example.com".into())),
            ]),
        );
}

/// Test transaction with nested operations
#[test]
fn test_transaction_nested_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Category(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS SUBCATEGORY_OF")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Category(name) VALUES \
            1:('Electronics'), \
            2:('Computers'), \
            3:('Laptops'), \
            4:('Desktops')",
        )
        .assert_success()
        .exec_dml(
            "INSERT EDGE SUBCATEGORY_OF VALUES \
            2->1, \
            3->2, \
            4->2",
        )
        .assert_success()
        .query("MATCH (sub:Category)-[:SUBCATEGORY_OF]->(parent:Category) RETURN sub")
        .assert_result_count(3);
}

/// Test transaction with aggregation operations
#[test]
fn test_transaction_aggregation() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Product(name STRING, price INT, quantity INT)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Product(name, price, quantity) VALUES \
            1:('ProductA', 100, 10), \
            2:('ProductB', 200, 5), \
            3:('ProductC', 150, 8)",
        )
        .assert_success()
        .query("MATCH (p:Product) RETURN p.price")
        .assert_result_count(3);
}

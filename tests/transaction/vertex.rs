//! Transaction Vertex Operation Tests
//!
//! Test coverage:
//! - Multiple vertices insertion in single transaction
//! - Vertex update in transaction
//! - Vertex deletion in transaction
//! - Batch insert performance in transaction
//! - Property types support

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use std::collections::HashMap;

/// Test multiple vertices insertion in single transaction
#[test]
fn test_transaction_multiple_vertices() {
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
        .query("MATCH (v:Person) RETURN v")
        .assert_result_count(3)
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

/// Test vertex update in transaction
#[test]
fn test_transaction_vertex_update() {
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
        )
        .exec_dml("UPDATE 1 SET name = 'AliceUpdated', age = 31")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("AliceUpdated".into())),
                ("age", Value::Int(31)),
            ]),
        );
}

/// Test vertex deletion in transaction
#[test]
fn test_transaction_vertex_delete() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_vertex_exists(2, "Person");
}

/// Test batch insert performance in transaction
#[test]
fn test_transaction_batch_insert() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space2")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(id INT, name STRING)")
        .assert_success();

    let mut values = Vec::new();
    for i in 1..=10 {
        values.push(format!("{}:({}, 'Person{}')", i, i, i));
    }
    let insert_query = format!(
        "INSERT VERTEX Person(id, name) VALUES {}",
        values.join(", ")
    );

    scenario
        .exec_dml(&insert_query)
        .assert_success()
        .query("MATCH (v:Person) RETURN v")
        .assert_result_count(10);
}

/// Test transaction with property types
#[test]
fn test_transaction_property_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            "CREATE TAG IF NOT EXISTS TestTypes( \
            int_val INT, \
            string_val STRING, \
            bool_val BOOL, \
            float_val FLOAT, \
            timestamp_val TIMESTAMP)",
        )
        .assert_success()
        .exec_dml(
            "INSERT VERTEX TestTypes(int_val, string_val, bool_val, float_val) \
            VALUES 1:(42, 'test', true, 2.71)",
        )
        .assert_success()
        .assert_vertex_props(
            1,
            "TestTypes",
            HashMap::from([
                ("int_val", Value::Int(42)),
                ("string_val", Value::String("test".into())),
                ("bool_val", Value::Bool(true)),
                ("float_val", Value::Float(2.71)),
            ]),
        );
}

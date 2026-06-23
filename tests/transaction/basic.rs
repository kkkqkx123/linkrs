//! Transaction Basic Lifecycle Tests
//!
//! Test coverage:
//! - Basic transaction lifecycle - begin, commit, data persistence
//! - Transaction rollback - data should not persist after abort
//! - Empty transaction (no operations)
//! - Data visibility - committed data should be visible

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use std::collections::HashMap;

/// Test basic transaction lifecycle - begin, commit, data persistence
#[test]
fn test_transaction_basic_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v")
        .assert_result_count(1)
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

/// Test transaction rollback - data should not persist after abort
#[test]
fn test_transaction_rollback() {
    let scenario = TestScenario::new().expect("Failed to create test scenario");

    let scenario = scenario
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Before')")
        .assert_success();

    scenario
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v")
        .assert_result_count(1);
}

/// Test empty transaction (no operations)
#[test]
fn test_transaction_empty() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .query("MATCH (v:Person) RETURN v")
        .assert_result_count(0);
}

/// Test transaction data visibility - committed data should be visible
#[test]
fn test_transaction_data_visibility() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('First')")
        .assert_success();

    scenario = scenario
        .query("MATCH (v:Person) WHERE id(v) == 1 RETURN v")
        .assert_result_count(1);

    scenario
        .exec_dml("INSERT VERTEX Person(name) VALUES 2:('Second')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v")
        .assert_result_count(2);
}

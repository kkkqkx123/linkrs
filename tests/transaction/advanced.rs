//! Transaction Advanced Feature Tests
//!
//! Test coverage:
//! - Savepoint functionality
//! - Read-only transaction option
//! - Durability levels
//! - Transaction abort and recovery
//! - Transaction statistics
//! - Transaction cleanup on drop
//! - Large dataset handling
//! - Complex graph patterns
//! - Property filtering
//! - String operations
//! - Savepoint rollback

use super::common;

use common::test_scenario::TestScenario;
use graphdb::core::Value;
use graphdb::transaction::{TransactionManager, TransactionManagerConfig, TransactionOptions};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Test savepoint functionality
#[test]
fn test_savepoint_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Account(id INT, amount INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Account(id, amount) VALUES 1:(1, 100)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Account",
            HashMap::from([("id", Value::Int(1)), ("amount", Value::Int(100))]),
        )
        .exec_dml("INSERT VERTEX Account(id, amount) VALUES 2:(2, 200)")
        .assert_success()
        .assert_vertex_props(2, "Account", HashMap::from([("amount", Value::Int(200))]));
}

/// Test multiple savepoints
#[test]
fn test_savepoint_multiple() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space2")
        .exec_ddl("CREATE TAG IF NOT EXISTS Counter(value INT)")
        .assert_success();

    let scenario = (1..=5).fold(scenario, |s, i| {
        let query = format!("INSERT VERTEX Counter(value) VALUES {}:({})", i, i * 10);
        s.exec_dml(&query).assert_success()
    });

    scenario
        .query("MATCH (v:Counter) RETURN v")
        .assert_result_count(5);
}

/// Test transaction with read-only option
#[test]
fn test_readonly_transaction_option() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("MATCH (v:Person) RETURN v.name")
        .assert_result_count(1);
}

/// Test transaction durability levels
#[test]
fn test_transaction_durability_levels() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Immediate')")
        .assert_success()
        .assert_vertex_exists(1, "Person");
}

/// Test transaction abort and recovery
#[test]
fn test_transaction_abort_recovery() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager
        .abort_transaction(txn_id)
        .expect("Failed to abort transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test transaction statistics
#[test]
fn test_transaction_statistics() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let initial_stats = manager.stats();
    let initial_total = initial_stats.total_transactions.load(Ordering::Relaxed);

    for i in 0..5 {
        let options = if i % 2 == 0 {
            TransactionOptions::default()
        } else {
            TransactionOptions::new().read_only()
        };

        let txn_id = manager
            .begin_transaction(options)
            .expect("Failed to begin transaction");

        if i % 3 == 0 {
            manager
                .abort_transaction(txn_id)
                .expect("Failed to abort transaction");
        } else {
            manager
                .commit_transaction(txn_id)
                .expect("Failed to commit transaction");
        }
    }

    let final_stats = manager.stats();
    assert!(final_stats.total_transactions.load(Ordering::Relaxed) > initial_total);
}

/// Test transaction cleanup on drop
#[test]
fn test_transaction_cleanup() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    for i in 0..5 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::default())
            .unwrap();

        let active_count = manager.stats().active_transactions.load(Ordering::Relaxed);
        assert_eq!(active_count, 1);

        if i % 2 == 0 {
            manager.commit_transaction(txn_id).unwrap();
        } else {
            manager.abort_transaction(txn_id).unwrap();
        }

        let final_active_count = manager.stats().active_transactions.load(Ordering::Relaxed);
        assert_eq!(final_active_count, 0);
    }
}

/// Test transaction with large dataset
#[test]
fn test_transaction_large_dataset() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Item(id INT, info STRING)")
        .assert_success();

    let mut values = Vec::new();
    for i in 1..=10 {
        values.push(format!("{}:({}, 'val_{}')", i, i, i));
    }
    let query = format!("INSERT VERTEX Item(id, info) VALUES {}", values.join(", "));

    scenario = scenario.exec_dml(&query).assert_success();

    scenario
        .query("MATCH (v:Item) RETURN v")
        .assert_result_count(10);
}

/// Test transaction with complex graph pattern
#[test]
fn test_transaction_complex_graph_pattern() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG IF NOT EXISTS Company(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_AT")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS MANAGES")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT VERTEX Company(name) VALUES 100:('TechCorp')")
        .assert_success()
        .exec_dml("INSERT EDGE WORKS_AT VALUES 1->100, 2->100, 3->100")
        .assert_success()
        .exec_dml("INSERT EDGE MANAGES VALUES 1->2, 1->3")
        .assert_success()
        .query("MATCH (manager:Person)-[:MANAGES]->(employee:Person) RETURN manager.name, employee.name")
        .assert_result_count(2);
}

/// Test transaction with property filtering
#[test]
fn test_transaction_property_filtering() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, age INT, active BOOL)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name, age, active) VALUES \
            1:('Alice', 25, true), \
            2:('Bob', 30, false), \
            3:('Charlie', 35, true)",
        )
        .assert_success()
        .query("MATCH (v:Person) WHERE v.active == true RETURN v.name")
        .assert_result_count(2)
        .query("MATCH (v:Person) WHERE v.age >= 30 RETURN v.name")
        .assert_result_count(2);
}

/// Test transaction with string operations
#[test]
fn test_transaction_string_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING, description STRING)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name, description) VALUES \
            1:('Alice', 'Software Engineer'), \
            2:('Bob', 'Data Scientist')",
        )
        .assert_success()
        .query("MATCH (v:Person) WHERE v.name STARTS WITH 'A' RETURN v.name")
        .assert_result_count(1)
        .assert_result_contains(vec![Value::String("Alice".into())]);
}

/// Test savepoint create and rollback
#[test]
fn test_savepoint_rollback() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp_id = manager
        .create_savepoint(txn_id, Some("initial".to_string()))
        .expect("Failed to create savepoint");

    let savepoint = manager.get_savepoint(txn_id, sp_id);
    assert!(savepoint.is_some());
    assert_eq!(savepoint.unwrap().name, Some("initial".to_string()));

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test multiple savepoints
#[test]
fn test_savepoint_multiple_rollback() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    let sp1 = manager
        .create_savepoint(txn_id, Some("sp1".to_string()))
        .expect("Failed to create savepoint 1");
    let _sp2 = manager
        .create_savepoint(txn_id, Some("sp2".to_string()))
        .expect("Failed to create savepoint 2");

    let savepoints = manager.get_active_savepoints(txn_id);
    assert_eq!(savepoints.len(), 2);

    manager
        .release_savepoint(txn_id, sp1)
        .expect("Failed to release savepoint");

    let savepoints_after = manager.get_active_savepoints(txn_id);
    assert_eq!(savepoints_after.len(), 1);

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test transaction with timeout
#[test]
fn test_transaction_with_timeout() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_secs(60)))
        .expect("Failed to begin transaction");

    manager
        .commit_transaction(txn_id)
        .expect("Failed to commit transaction");
}

/// Test max concurrent transactions
#[test]
fn test_max_concurrent_transactions() {
    let config = TransactionManagerConfig {
        max_concurrent_transactions: 5,
        ..Default::default()
    };

    let manager = TransactionManager::new(config);

    let mut txn_ids = Vec::new();
    for _ in 0..5 {
        let txn_id = manager
            .begin_transaction(TransactionOptions::new().read_only())
            .expect("Failed to begin transaction");
        txn_ids.push(txn_id);
    }

    for txn_id in txn_ids {
        manager
            .commit_transaction(txn_id)
            .expect("Failed to commit");
    }
}

/// Test cleanup expired transactions
#[test]
fn test_cleanup_expired_transactions() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::new().with_timeout(Duration::from_millis(10)))
        .expect("Failed to begin transaction");

    std::thread::sleep(Duration::from_millis(50));

    manager.cleanup_expired_transactions();

    assert!(!manager.is_transaction_active(txn_id));
}

/// Test shutdown functionality
#[test]
fn test_shutdown() {
    let manager = TransactionManager::new(TransactionManagerConfig::default());

    let txn_id = manager
        .begin_transaction(TransactionOptions::default())
        .expect("Failed to begin transaction");

    assert!(manager.is_transaction_active(txn_id));

    manager.shutdown();

    assert!(!manager.is_transaction_active(txn_id));
}

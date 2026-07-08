#![cfg(feature = "embedded")]

use graphdb::api::embedded::{GraphDatabase, TransactionConfig};
use std::time::Duration;

#[test]
fn test_add_property_rollback_restores_schema() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let txn = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string)").expect("Create tag should succeed");
    txn.execute("ALTER TAG user ADD PROPERTY age int").expect("Add property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin second transaction");
    let result = txn2.execute("MATCH (v:user) RETURN v.age").expect("Query should succeed");

    assert_eq!(result.len(), 0, "Property 'age' should not exist after rollback");

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_delete_property_rollback_restores_property() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let txn = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string, age int)").expect("Create tag should succeed");
    txn.execute("INSERT VERTEX user(name, age) VALUES \"1\":(\"Alice\", 25)").expect("Insert should succeed");
    txn.execute("ALTER TAG user DROP PROPERTY age").expect("Drop property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin second transaction");
    let result = txn2.execute("MATCH (v:user) RETURN v.age").expect("Query should succeed after rollback");

    assert!(!result.is_empty(), "Property 'age' should be restored after rollback");

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_edge_property_rollback() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let txn = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string)").expect("Create user tag should succeed");
    txn.execute("CREATE EDGE likes(weight int)").expect("Create edge should succeed");
    txn.execute("INSERT VERTEX user(name) VALUES \"alice\":(\"Alice\")").expect("Insert user should succeed");
    txn.execute("INSERT VERTEX user(name) VALUES \"bob\":(\"Bob\")").expect("Insert user should succeed");
    txn.execute("INSERT EDGE likes(weight) FROM \"alice\" TO \"bob\" VALUES 100").expect("Insert edge should succeed");

    txn.execute("ALTER EDGE likes DROP PROPERTY weight").expect("Drop property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session.begin_transaction_with_config(TransactionConfig::new()
        .with_timeout(Duration::from_secs(60))).expect("Failed to begin second transaction");
    let result = txn2.execute("MATCH (a:user)-[e:likes]->(b:user) RETURN e.weight").expect("Query should succeed");

    assert!(!result.is_empty(), "Edge property 'weight' should be restored after rollback");

    txn2.commit().expect("Commit should succeed");
}

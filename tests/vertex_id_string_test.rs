use graphdb::api::embedded::{GraphDatabase, TransactionConfig};
use std::time::Duration;

#[test]
fn test_string_vertex_id_insert_and_query() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string)").expect("Create tag should succeed");
    txn.execute("INSERT VERTEX user(name) VALUES \"alice\":(\"Alice Smith\")").expect("Insert with string ID should succeed");

    let result = txn.execute("MATCH (v:user) WHERE v.name == \"Alice Smith\" RETURN v").expect("Query should succeed");
    assert!(!result.is_empty(), "Should find the inserted vertex");

    txn.commit().expect("Commit should succeed");
}

#[test]
fn test_string_vertex_id_delete_and_rollback() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60));

    let txn = session.begin_transaction_with_config(config.clone()).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string)").expect("Create tag should succeed");
    txn.execute("INSERT VERTEX user(name) VALUES \"alice\":(\"Alice Smith\")").expect("Insert should succeed");
    txn.execute("DELETE VERTEX \"alice\"").expect("Delete should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session.begin_transaction_with_config(config).expect("Failed to begin second transaction");
    let _result = txn2.execute("MATCH (v:user) WHERE v.name == \"Alice Smith\" RETURN v").expect("Query should succeed after rollback");

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_string_vertex_id_update_and_rollback() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60));

    let txn = session.begin_transaction_with_config(config.clone()).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string, age int)").expect("Create tag should succeed");
    txn.execute("INSERT VERTEX user(name, age) VALUES \"alice\":(\"Alice Smith\", 25)").expect("Insert should succeed");
    txn.execute("UPDATE VERTEX \"alice\" SET age = 26").expect("Update should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session.begin_transaction_with_config(config).expect("Failed to begin second transaction");
    let _result = txn2.execute("MATCH (v:user) WHERE v.name == \"Alice Smith\" RETURN v.age").expect("Query should succeed after rollback");

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_mixed_vertex_id_types() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(name string)").expect("Create tag should succeed");

    txn.execute("INSERT VERTEX user(name) VALUES 1:(\"Alice\")").expect("Insert with int ID should succeed");
    txn.execute("INSERT VERTEX user(name) VALUES \"bob\":(\"Bob\")").expect("Insert with string ID should succeed");

    let result = txn.execute("MATCH (v:user) RETURN v.name ORDER BY v.name").expect("Query should succeed");
    assert_eq!(result.len(), 2, "Should find both vertices");

    txn.commit().expect("Commit should succeed");
}

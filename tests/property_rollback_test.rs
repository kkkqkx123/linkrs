#![cfg(feature = "embedded")]

use linkrs::api::api_core::SpaceConfig;
use linkrs::api::embedded::{GraphDatabase, TransactionConfig};
use std::time::Duration;

#[test]
fn test_add_property_rollback_restores_schema() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let mut session = db.session().expect("Failed to create session");
    session
        .create_space("test_space", SpaceConfig::default())
        .expect("Failed to create space");
    session
        .use_space("test_space")
        .expect("Failed to use space");

    let txn = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin transaction");

    txn.execute("CREATE TAG user(id string, name string)")
        .expect("Create tag should succeed");
    txn.execute("ALTER TAG user ADD (age int)")
        .expect("Add property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin second transaction");
    let result = txn2
        .execute("MATCH (v:user) RETURN v.age")
        .expect("Query should succeed");

    assert_eq!(
        result.len(),
        0,
        "Property 'age' should not exist after rollback"
    );

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_delete_property_rollback_restores_property() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let mut session = db.session().expect("Failed to create session");
    session
        .create_space("test_space", SpaceConfig::default())
        .expect("Failed to create space");
    session
        .use_space("test_space")
        .expect("Failed to use space");

    let setup = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin setup transaction");

    setup
        .execute("CREATE TAG user(id string, name string, age int)")
        .expect("Create tag should succeed");
    setup
        .execute("INSERT VERTEX user(id, name, age) VALUES \"1\":(\"1\", \"Alice\", 25)")
        .expect("Insert should succeed");
    setup.commit().expect("Setup commit should succeed");

    let txn = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin transaction");
    txn.execute("ALTER TAG user DROP (age)")
        .expect("Drop property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin second transaction");
    let result = txn2
        .execute("MATCH (v:user) RETURN v.age")
        .expect("Query should succeed after rollback");

    assert!(
        !result.is_empty(),
        "Property 'age' should be restored after rollback"
    );

    txn2.commit().expect("Commit should succeed");
}

#[test]
fn test_edge_property_rollback() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let mut session = db.session().expect("Failed to create session");
    session
        .create_space("test_space", SpaceConfig::default())
        .expect("Failed to create space");
    session
        .use_space("test_space")
        .expect("Failed to use space");

    let txn = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin transaction");

    let setup = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin setup transaction");
    setup
        .execute("CREATE TAG user(id string, name string)")
        .expect("Create user tag should succeed");
    setup
        .execute("CREATE EDGE likes(weight int)")
        .expect("Create edge should succeed");
    setup
        .execute("INSERT VERTEX user(id, name) VALUES \"alice\":(\"alice\", \"Alice\")")
        .expect("Insert user should succeed");
    setup
        .execute("INSERT VERTEX user(id, name) VALUES \"bob\":(\"bob\", \"Bob\")")
        .expect("Insert user should succeed");
    setup
        .execute("INSERT EDGE likes(weight) VALUES \"alice\"->\"bob\":(100)")
        .expect("Insert edge should succeed");
    setup.commit().expect("Setup commit should succeed");

    let txn = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin transaction");
    txn.execute("ALTER EDGE likes DROP (weight)")
        .expect("Drop property should succeed");

    txn.rollback().expect("Rollback should succeed");

    let txn2 = session
        .begin_transaction_with_config(
            TransactionConfig::new().with_timeout(Duration::from_secs(60)),
        )
        .expect("Failed to begin second transaction");
    let result = txn2
        .execute("MATCH (a:user)-[e:likes]->(b:user) RETURN e.weight")
        .expect("Query should succeed");

    assert!(
        !result.is_empty(),
        "Edge property 'weight' should be restored after rollback"
    );

    txn2.commit().expect("Commit should succeed");
}

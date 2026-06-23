use graphdb::api::embedded::{GraphDatabase, TransactionConfig};
use std::time::Duration;

#[test]
fn test_transaction_timeout_enforced_on_execute() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_millis(100));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    std::thread::sleep(Duration::from_millis(150));

    let result = txn.execute("CREATE TAG test(name string)");
    assert!(result.is_err(), "Transaction should fail due to timeout");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timeout") || err_msg.contains("Timeout"),
        "Error should mention timeout, got: {}",
        err_msg
    );
}

#[test]
fn test_transaction_timeout_enforced_on_commit() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_millis(100));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    txn.execute("CREATE TAG test(name string)").expect("Execute should succeed before timeout");

    std::thread::sleep(Duration::from_millis(150));

    let result = txn.commit();
    assert!(result.is_err(), "Commit should fail due to timeout");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timeout") || err_msg.contains("Timeout"),
        "Error should mention timeout, got: {}",
        err_msg
    );
}

#[test]
fn test_transaction_no_timeout_executes_successfully() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_timeout(Duration::from_secs(60));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    txn.execute("CREATE TAG test(name string)").expect("Execute should succeed");
    txn.execute("INSERT VERTEX test(name) VALUES \"1\":(\"Alice\")").expect("Insert should succeed");

    txn.commit().expect("Commit should succeed");
}

#[test]
fn test_idle_timeout_enforced() {
    let db = GraphDatabase::open_in_memory().expect("Failed to open database");
    let session = db.session().expect("Failed to create session");

    let config = TransactionConfig::new()
        .with_idle_timeout(Duration::from_millis(100));

    let txn = session.begin_transaction_with_config(config).expect("Failed to begin transaction");

    txn.execute("CREATE TAG test(name string)").expect("First execute should succeed");

    std::thread::sleep(Duration::from_millis(150));

    let result = txn.execute("INSERT VERTEX test(name) VALUES \"1\":(\"Alice\")");
    assert!(result.is_err(), "Execute should fail due to idle timeout");
}

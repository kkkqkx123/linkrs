//! Session-level transaction E2E tests
//!
//! Covers the verification backlog from the transaction snapshot plan:
//! - `BEGIN READ ONLY` session-level end-to-end: two statements within one
//!   read-only transaction observe the same snapshot
//! - DML inside a read-only transaction is rejected
//! - SAVEPOINT / ROLLBACK TO SAVEPOINT / RELEASE SAVEPOINT through the real
//!   TransactionManager + storage undo path (not a mock)

use crate::common::{assert_query_err, assert_query_ok, create_test_db, setup_test_space};

/// A single MATCH count over the `person` tag.
fn count_persons(db: &mut crate::common::TestDb, tag_filter: &str) -> usize {
    let result = db.execute_query(&format!("MATCH (p:{}) RETURN p.name", tag_filter));
    let result = result.expect("MATCH should succeed");
    result.rows().len()
}

/// `BEGIN READ ONLY` session-level end-to-end.
///
/// Inserts committed before the transaction are visible; a vertex committed
/// after the transaction started must NOT be visible to either statement of
/// the read-only transaction (consistent snapshot), and becomes visible only
/// after COMMIT.
#[test]
fn test_begin_read_only_consistent_snapshot_across_statements() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_readonly_snapshot",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Committed before the transaction starts.
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("pre-transaction INSERT should succeed");

    // Start a read-only transaction.
    let result = db.execute_query("BEGIN READ ONLY");
    assert_query_ok(result, "BEGIN READ ONLY should succeed");

    // Statement 1 inside the transaction: pre-existing vertex is visible.
    assert_eq!(
        count_persons(&mut db, "person"),
        1,
        "statement 1 should see p1"
    );

    // Concurrent auto-commit write from another session commits after the
    // snapshot was taken.
    db.execute_external("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("external INSERT should succeed");

    // Statement 2 inside the transaction: the externally committed vertex
    // must NOT be visible — both statements share the snapshot.
    assert_eq!(
        count_persons(&mut db, "person"),
        1,
        "statement 2 must see the same snapshot as statement 1 (p2 invisible)"
    );

    // Commit ends the snapshot; the new vertex becomes visible.
    let result = db.execute_query("COMMIT");
    assert_query_ok(result, "COMMIT should succeed");
    assert_eq!(
        count_persons(&mut db, "person"),
        2,
        "post-COMMIT should see p2"
    );
}

/// DML inside a `BEGIN READ ONLY` transaction must be rejected.
#[test]
fn test_read_only_transaction_rejects_dml() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_readonly_dml",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("BEGIN READ ONLY");
    assert_query_ok(result, "BEGIN READ ONLY should succeed");

    let result = db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)");
    assert_query_err(result, "INSERT inside a read-only transaction must fail");

    // The transaction is still usable for reads after the rejected write.
    assert_eq!(
        count_persons(&mut db, "person"),
        0,
        "no data may be written"
    );

    let result = db.execute_query("ROLLBACK");
    assert_query_ok(result, "ROLLBACK should succeed");

    // Data written after the transaction is unaffected.
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT after ROLLBACK should succeed");
    assert_eq!(count_persons(&mut db, "person"), 1);
}

/// SAVEPOINT / ROLLBACK TO SAVEPOINT / RELEASE SAVEPOINT through the real
/// transaction + storage undo path.
#[test]
fn test_savepoint_rollback_to_restores_data() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_savepoint",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("BEGIN");
    assert_query_ok(result, "BEGIN should succeed");

    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT p1 should succeed");

    let result = db.execute_query("SAVEPOINT sp1");
    assert_query_ok(result, "SAVEPOINT sp1 should succeed");

    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT p2 should succeed");
    assert_eq!(count_persons(&mut db, "person"), 2, "both vertices visible");

    // Roll back to the savepoint: p2 must be undone, p1 must remain.
    let result = db.execute_query("ROLLBACK TO sp1");
    assert_query_ok(result, "ROLLBACK TO sp1 should succeed");
    assert_eq!(
        count_persons(&mut db, "person"),
        1,
        "post-savepoint write must be undone"
    );

    // Release the savepoint (still valid after rollback-to).
    let result = db.execute_query("RELEASE SAVEPOINT sp1");
    assert_query_ok(result, "RELEASE SAVEPOINT sp1 should succeed");

    // Data written after rollback-to is retained on COMMIT.
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT p2 again should succeed");

    let result = db.execute_query("COMMIT");
    assert_query_ok(result, "COMMIT should succeed");
    assert_eq!(count_persons(&mut db, "person"), 2, "post-COMMIT state");
}

/// ROLLBACK TO a released savepoint must fail.
#[test]
fn test_rollback_to_released_savepoint_fails() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_savepoint_release",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("BEGIN");
    assert_query_ok(result, "BEGIN should succeed");

    let result = db.execute_query("SAVEPOINT sp1");
    assert_query_ok(result, "SAVEPOINT sp1 should succeed");
    let result = db.execute_query("RELEASE SAVEPOINT sp1");
    assert_query_ok(result, "RELEASE SAVEPOINT sp1 should succeed");

    let result = db.execute_query("ROLLBACK TO sp1");
    assert_query_err(result, "ROLLBACK TO a released savepoint must fail");

    let result = db.execute_query("ROLLBACK");
    assert_query_ok(result, "ROLLBACK should succeed");
}

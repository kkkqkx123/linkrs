//! E2E tests for EXISTS / IN / NOT IN subqueries (P1).
//!
//! Exercises the conjunctive-WHERE transformation: the parser+binder let
//! subqueries through, the exists planner turns them into PatternApply (or
//! after decorrelation SemiJoin/AntiJoin), and the runtime returns correct
//! rows for correlated and uncorrelated subqueries.

use crate::common::{create_test_db, setup_test_space};
use graphdb::core::Value;

fn setup_person_graph(db: &mut crate::common::TestDb) {
    setup_test_space(
        db,
        "e2e_subquery",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p3': ('Carol', 35)")
        .expect("INSERT should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p4': ('Dave', 20)")
        .expect("INSERT should succeed");

    db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2': (0.8)")
        .expect("INSERT EDGE should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p2' -> 'p3': (0.7)")
        .expect("INSERT EDGE should succeed");
}

/// Correlated EXISTS with an equality key:
/// `p.name == t.name` → hash=t.name, probe=p.name → semi join.
#[test]
fn test_exists_correlated_equality() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.name == t.name } RETURN t.name",
    );
    let result = result.expect("correlated EXISTS should execute and succeed");
    assert_eq!(result.rows.len(), 4, "every person matches itself");
}

/// Uncorrelated EXISTS (no key): PatternApply with empty keys behaves as
/// "right table non-empty".
#[test]
fn test_exists_uncorrelated() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age == 30 } RETURN t.name",
    );
    let result = result.expect("uncorrelated EXISTS should execute and succeed");
    assert_eq!(result.rows.len(), 4, "someone has age 30, so all match");
}

/// NOT EXISTS with a subquery-local filter: nobody has age 100, so the anti
/// join keeps everyone.
#[test]
fn test_not_exists() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE NOT EXISTS { MATCH (p:person) WHERE p.age == 100 } RETURN t.name",
    )
    .expect("NOT EXISTS should execute and succeed");
    assert_eq!(result.rows.len(), 4, "nobody has age 100, so all match");
}

/// Correlated NOT EXISTS with a key: only Carol satisfies `p.name == t.name
/// AND p.age == 35`, so the anti join drops Carol.
#[test]
fn test_not_exists_correlated_key() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE NOT EXISTS { MATCH (p:person) WHERE p.name == t.name AND p.age == 35 } RETURN t.name",
    )
    .expect("correlated NOT EXISTS should execute and succeed");
    assert_eq!(result.rows.len(), 3, "Carol is excluded");
}

/// IN subquery: the synthesized equality `t.name == p.name` forms the key.
#[test]
fn test_in_subquery() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE t.name IN { MATCH (p:person) RETURN p.name } RETURN t.name",
    );
    let result = result.expect("IN subquery should execute and succeed");
    assert_eq!(result.rows.len(), 4, "every name appears in the result set");
}

/// NOT IN subquery: anti join against the projected names.
#[test]
fn test_not_in_subquery() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE t.name NOT IN { MATCH (p:person) WHERE p.age == 35 RETURN p.name } RETURN t.name",
    );
    let result = result.expect("NOT IN subquery should execute and succeed");
    assert_eq!(result.rows.len(), 3, "Carol is excluded");
}

/// EXISTS with a graph path subquery: only vertices with an outgoing friend
/// edge match.
#[test]
fn test_exists_with_path_subquery() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE EXISTS { MATCH (p:person)-[:friend]->(q:person) WHERE p.name == t.name } RETURN t.name",
    );
    let result = result.expect("path EXISTS should execute and succeed");
    assert_eq!(result.rows.len(), 2, "Alice and Bob have outgoing friend edges");
}

/// The residual (non-subquery) condition stays a filter on the outer plan.
#[test]
fn test_exists_with_residual_condition() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "MATCH (t:person) WHERE t.age > 20 AND EXISTS { MATCH (p:person) WHERE p.age == 30 } RETURN t.name",
    );
    let result = result.expect("EXISTS with residual condition should execute and succeed");
    assert_eq!(result.rows.len(), 3, "Alice, Bob, Carol (Dave is 20)");
}

/// EXPLAIN surfaces the subquery operator: PatternApply (pre-decorrelation)
/// or SemiJoin / AntiJoin (after decorrelation).
#[test]
fn test_explain_shows_subquery_operator() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db.execute_query(
        "EXPLAIN MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.name == t.name } RETURN t.name",
    );
    let result = result.expect("EXPLAIN should execute and succeed");

    let joined: String = result
        .rows
        .iter()
        .flat_map(|row| row.values.values())
        .filter_map(|v| match v {
            Value::String(s) => Some(s.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        joined.contains("PatternApply") || joined.contains("SemiJoin"),
        "expected PatternApply or SemiJoin in EXPLAIN output, got: {}",
        joined
    );
}

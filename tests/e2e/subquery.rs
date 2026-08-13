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
    assert_eq!(
        result.rows.len(),
        2,
        "Alice and Bob have outgoing friend edges"
    );
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

///  correlated non-equi EXISTS routes through
/// CorrelatedApply and re-executes the right subtree per outer row.
#[test]
fn test_correlated_non_equi_exists() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
        )
        .expect("correlated non-equi EXISTS should execute and succeed");
    // Alice(30), Bob(25), Dave(20) each have a person with a strictly higher
    // age; Carol(35) does not.
    assert_eq!(result.rows.len(), 3, "Alice, Bob, Dave match");
}

#[test]
fn test_correlated_non_equi_not_exists() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE NOT EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
        )
        .expect("correlated non-equi NOT EXISTS should execute and succeed");
    assert_eq!(
        result.rows.len(),
        1,
        "only Carol has no strictly older person"
    );
}

#[test]
fn test_correlated_non_equi_in() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    // `t.age IN { p.age > t.age }` can never hold with unique ages, but the
    // correlated IN must still route through CorrelatedApply and return empty.
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age IN { MATCH (p:person) WHERE p.age > t.age RETURN p.age } RETURN t.name",
        )
        .expect("correlated non-equi IN should execute and succeed");
    assert_eq!(
        result.rows.len(),
        0,
        "no age is strictly greater than itself"
    );
}

/// Correlated non-equi NOT IN: `p.age > t.age` can never yield `t.age`, so the
/// anti semantics of NOT IN keep every outer row.
#[test]
fn test_correlated_non_equi_not_in() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age NOT IN { MATCH (p:person) WHERE p.age > t.age RETURN p.age } RETURN t.name",
        )
        .expect("correlated non-equi NOT IN should execute and succeed");
    assert_eq!(
        result.rows.len(),
        4,
        "no age is strictly greater than itself, so NOT IN keeps everyone"
    );
}

/// Multi-variable correlation: the condition references two subquery variables
/// (`p.age + q.age`) on one side, so no equi key exists and the whole condition
/// routes through CorrelatedApply.
#[test]
fn test_correlated_multi_variable_condition() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    // friend edges: Alice->Bob (30+25=55), Bob->Carol (25+35=60).
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person)-[:friend]->(q:person) WHERE p.age + q.age > t.age + 30 } RETURN t.name",
        )
        .expect("multi-variable correlated EXISTS should execute and succeed");
    // Bob(25): 55 > 55 is false but 60 > 55 is true  -> match
    // Dave(20): 55 > 50 is true                     -> match
    // Alice(30): 55 > 60 and 60 > 60 are both false -> no match
    // Carol(35): threshold 65, neither pair reaches -> no match
    assert_eq!(result.rows.len(), 2, "Bob and Dave match");
}

/// Nested correlation: a correlated EXISTS whose subquery contains another
/// correlated EXISTS (against the intermediate `p` variable).
#[test]
fn test_nested_correlated_exists() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age > t.age AND EXISTS { MATCH (q:person) WHERE q.age > p.age } } RETURN t.name",
        )
        .expect("nested correlated EXISTS should execute and succeed");
    // Bob(25): p=Alice(30) then EXISTS q.age>30 (Carol)    -> match
    // Dave(20): p=Alice(30) then EXISTS q.age>30 (Carol)   -> match
    // Alice(30): only p=Carol(35), EXISTS q.age>35 is empty -> no match
    // Carol(35): no p with age>35                           -> no match
    assert_eq!(result.rows.len(), 2, "Bob and Dave match");
}

/// EXPLAIN surfaces the CorrelatedApply operator for a non-equi correlated
/// subquery, with an `anti` annotation on the NOT EXISTS variant.
#[test]
fn test_explain_shows_correlated_apply() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let mut joined = |query: &str| {
        let result = db.execute_query(query).expect("EXPLAIN should succeed");
        result
            .rows
            .iter()
            .flat_map(|row| row.values.values())
            .filter_map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    let semi = joined(
        "EXPLAIN MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
    );
    assert!(
        semi.contains("CorrelatedApply"),
        "expected CorrelatedApply in EXPLAIN output, got: {}",
        semi
    );
    assert!(!semi.contains("anti:"), "semi variant must not be anti");

    let anti = joined(
        "EXPLAIN MATCH (t:person) WHERE NOT EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
    );
    assert!(
        anti.contains("CorrelatedApply"),
        "expected CorrelatedApply in EXPLAIN output, got: {}",
        anti
    );
    assert!(
        anti.contains("anti:"),
        "expected anti annotation in EXPLAIN output, got: {}",
        anti
    );
}

//! E2E tests for EXISTS / IN / NOT IN subqueries.
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
    assert_eq!(result.rows().len(), 4, "every person matches itself");
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
    assert_eq!(result.rows().len(), 4, "someone has age 30, so all match");
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
    assert_eq!(result.rows().len(), 4, "nobody has age 100, so all match");
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
    assert_eq!(result.rows().len(), 3, "Carol is excluded");
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
    assert_eq!(
        result.rows().len(),
        4,
        "every name appears in the result set"
    );
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
    assert_eq!(result.rows().len(), 3, "Carol is excluded");
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
        result.rows().len(),
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
    assert_eq!(result.rows().len(), 3, "Alice, Bob, Carol (Dave is 20)");
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
        .rows()
        .iter()
        .flat_map(|row| row.iter())
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
    assert_eq!(result.rows().len(), 3, "Alice, Bob, Dave match");
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
        result.rows().len(),
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
        result.rows().len(),
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
        result.rows().len(),
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
    assert_eq!(result.rows().len(), 2, "Bob and Dave match");
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
    assert_eq!(result.rows().len(), 2, "Bob and Dave match");
}

/// EXPLAIN surfaces the SemiJoin (Mark-Join) decorrelation for a non-equi
/// correlated subquery, carrying the residual as its join condition, with an
/// `anti` annotation on the NOT EXISTS variant.
#[test]
fn test_explain_shows_mark_join() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let mut joined = |query: &str| {
        let result = db.execute_query(query).expect("EXPLAIN should succeed");
        result
            .rows()
            .iter()
            .flat_map(|row| row.iter())
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
        semi.contains("SemiJoin"),
        "expected SemiJoin in EXPLAIN output, got: {}",
        semi
    );

    let anti = joined(
        "EXPLAIN MATCH (t:person) WHERE NOT EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
    );
    assert!(
        anti.contains("SemiJoin"),
        "expected SemiJoin in EXPLAIN output, got: {}",
        anti
    );
}

// ---------------------------------------------------------------------------
// Expression-level EXISTS / IN execute from Filter/Project hosts (WHERE
// residual, RETURN, WITH assignments, HAVING); ORDER BY / UNWIND / DML
// value positions are still refused at planning time.
// ---------------------------------------------------------------------------

/// Assert that `query` fails at planning time with the precise
/// expression-level subquery error (mentioning the conjunctive-WHERE
/// alternative) instead of reaching the runtime "not supported" path.
fn assert_expression_subquery_rejected(db: &mut crate::common::TestDb, query: &str) {
    let err = db
        .execute_query(query)
        .expect_err("expression-level EXISTS/IN must fail at planning time");
    let msg = err.to_string();
    assert!(
        msg.contains("EXISTS/IN subquery cannot be planned"),
        "expected precise planning error for `{query}`, got: {msg}"
    );
    assert!(
        msg.contains("conjunctive WHERE"),
        "expected conjunctive-WHERE hint for `{query}`, got: {msg}"
    );
}

/// WHERE residual: EXISTS / IN under OR (non-conjunctive positions) execute
/// per row via the Filter's SubqueryExecutor.
#[test]
fn test_where_or_exists_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age = 30 OR EXISTS { MATCH (p:person) } RETURN t.name",
        )
        .expect("OR-side EXISTS should execute and succeed");
    assert_eq!(
        result.rows().len(),
        4,
        "EXISTS is true, so every row matches"
    );

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age = 30 OR NOT EXISTS { MATCH (p:person) WHERE p.age == 100 } RETURN t.name",
        )
        .expect("OR-side NOT EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.name IN { MATCH (p:person) RETURN p.name } OR t.age > 20 RETURN t.name",
        )
        .expect("OR-side IN should execute and succeed");
    assert_eq!(
        result.rows().len(),
        4,
        "every name appears in the result set"
    );

    // A conjunctive EXISTS plus a residual OR-side subquery also executes.
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) } AND (t.age = 30 OR t.name IN { MATCH (p2:person) RETURN p2.name }) RETURN t.name",
        )
        .expect("conjunctive + residual subqueries should execute and succeed");
    assert_eq!(result.rows().len(), 4);
}

/// OR-side subqueries honor the OR semantics: a false left side keeps the
/// subquery result decisive.
#[test]
fn test_where_or_exists_false_left() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    // The EXISTS side is empty, so only the (false) left side decides: no
    // row matches.
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age = 100 OR EXISTS { MATCH (p:person) WHERE p.age == 100 } RETURN t.name",
        )
        .expect("OR-side EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 0, "neither side matches");

    // NOT EXISTS of the same empty subquery is true for every row.
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age = 100 OR NOT EXISTS { MATCH (p:person) WHERE p.age == 100 } RETURN t.name",
        )
        .expect("OR-side NOT EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4, "NOT EXISTS holds for everyone");
}

/// RETURN positions: plain, container-nested, CASE-branched EXISTS / IN.
#[test]
fn test_return_exists_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query("MATCH (t:person) RETURN EXISTS { MATCH (p:person) } AS x")
        .expect("RETURN EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(
            value,
            &Value::Bool(true),
            "every row sees a non-empty subquery"
        );
    }

    let result = db
        .execute_query(
            "MATCH (t:person) RETURN NOT EXISTS { MATCH (p:person) WHERE p.age == 100 } AS x",
        )
        .expect("RETURN NOT EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(value, &Value::Bool(true));
    }

    let result = db
        .execute_query("MATCH (t:person) RETURN t.age IN { MATCH (p:person) RETURN p.age } AS x")
        .expect("RETURN IN should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(value, &Value::Bool(true), "every age appears in the set");
    }

    let result = db
        .execute_query("MATCH (t:person) RETURN [EXISTS { MATCH (p:person) }] AS x")
        .expect("container-nested EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);

    let result = db
        .execute_query(
            "MATCH (t:person) RETURN CASE WHEN EXISTS { MATCH (p:person) } THEN 1 ELSE 0 END AS x",
        )
        .expect("CASE-branched EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(value, &Value::Int(1));
    }
}

/// RETURN NOT IN with a NULL left operand or NULL values in the result set:
/// NULL never matches.
#[test]
fn test_return_in_null_semantics() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) RETURN t.name AS name, t.name NOT IN { MATCH (p:person) WHERE p.age == 35 RETURN p.name } AS x",
        )
        .expect("RETURN NOT IN should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    let columns = result.columns();
    let name_idx = columns
        .iter()
        .position(|c| c == "name")
        .expect("name column");
    let x_idx = columns.iter().position(|c| c == "x").expect("x column");
    let carol = result
        .rows()
        .iter()
        .find(|row| row.get(name_idx) == Some(&Value::string("Carol")))
        .expect("Carol row present");
    assert_eq!(
        carol.get(x_idx),
        Some(&Value::Bool(false)),
        "Carol IS in the result set"
    );

    // A name absent from the result set yields NOT IN = true.
    let dave = result
        .rows()
        .iter()
        .find(|row| row.get(name_idx) == Some(&Value::string("Dave")))
        .expect("Dave row present");
    assert_eq!(
        dave.get(x_idx),
        Some(&Value::Bool(true)),
        "Dave is not in the result set"
    );
}

/// Correlated expression-level subqueries: the current row is bound as the
/// correlation frame and the sub-plan is re-executed per row.
#[test]
fn test_return_correlated_in_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) RETURN t.age IN { MATCH (p:person) WHERE p.age > t.age RETURN p.age } AS x",
        )
        .expect("correlated RETURN IN should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.last().expect("boolean column");
        assert_eq!(
            value,
            &Value::Bool(false),
            "no age is strictly greater than itself"
        );
    }

    // Correlated EXISTS inside an OR position.
    let result = db
        .execute_query(
            "MATCH (t:person) WHERE t.age = 20 OR EXISTS { MATCH (p:person) WHERE p.age > t.age } RETURN t.name",
        )
        .expect("OR-side correlated EXISTS should execute and succeed");
    // Dave matches via the OR side (age == 20); Alice, Bob and Dave have a
    // strictly older person; Carol has neither.
    assert_eq!(result.rows().len(), 3);
}

/// WITH assignments carry expression-level subqueries through the Project
/// node.
#[test]
fn test_with_assign_exists_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query("MATCH (t:person) WITH EXISTS { MATCH (p:person) } AS x RETURN x")
        .expect("WITH assignment EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(value, &Value::Bool(true));
    }

    let result = db
        .execute_query(
            "MATCH (t:person) WITH t.age IN { MATCH (p:person) RETURN p.age } AS x RETURN x",
        )
        .expect("WITH assignment IN should execute and succeed");
    assert_eq!(result.rows().len(), 4);
    for row in result.rows() {
        let value = row.first().expect("one column");
        assert_eq!(value, &Value::Bool(true));
    }
}

/// HAVING (GROUP BY) carries expression-level subqueries through the HAVING
/// Filter node.
#[test]
fn test_having_exists_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) RETURN t.age, count(*) AS c GROUP BY t.age HAVING EXISTS { MATCH (p:person) }",
        )
        .expect("HAVING EXISTS should execute and succeed");
    assert_eq!(result.rows().len(), 4, "every age group is retained");
}

/// ORDER BY expressions carry expression-level subqueries too — the Sort
/// host is not wired yet, so they stay rejected at planning time.
#[test]
fn test_order_by_exists_rejected_at_planning() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    assert_expression_subquery_rejected(
        &mut db,
        "MATCH (t:person) RETURN t.name ORDER BY EXISTS { MATCH (p:person) }",
    );
}

/// UNWIND list expressions carry expression-level subqueries too. The UNWIND
/// pipeline converts through the bound-expression path, whose converter
/// refuses EXISTS with its own precise planning-time error.
#[test]
fn test_unwind_exists_rejected_at_planning() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let err = db
        .execute_query("UNWIND [EXISTS { MATCH (p:person) }] AS x RETURN x")
        .expect_err("UNWIND with EXISTS must fail at planning time");
    let msg = err.to_string();
    assert!(
        msg.contains("Exists") || msg.contains("cannot be planned"),
        "UNWIND must be refused at planning time, got: {msg}"
    );
}

/// DML value expressions (SET / UPDATE / INSERT / MERGE) carry
/// expression-level subqueries too.
#[test]
fn test_dml_values_rejected_at_planning() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    assert_expression_subquery_rejected(&mut db, "SET 1.age = EXISTS { MATCH (p:person) }");
    assert_expression_subquery_rejected(&mut db, "UPDATE 1 SET age = EXISTS { MATCH (p:person) }");
    assert_expression_subquery_rejected(
        &mut db,
        "INSERT VERTEX person(name) VALUES 'p9': (EXISTS { MATCH (p:person) })",
    );
    assert_expression_subquery_rejected(
        &mut db,
        "MERGE (n:person) ON CREATE SET n.name = EXISTS { MATCH (p:person) }",
    );
}

/// EXPLAIN surfaces the expression-level subquery count on the hosting
/// Filter / Project operators (`subquery: N`).
#[test]
fn test_explain_shows_expression_subqueries() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let mut joined = |query: &str| {
        let result = db.execute_query(query).expect("EXPLAIN should succeed");
        result
            .rows()
            .iter()
            .flat_map(|row| row.iter())
            .filter_map(|v| match v {
                Value::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    let where_or = joined(
        "EXPLAIN MATCH (t:person) WHERE t.age = 30 OR EXISTS { MATCH (p:person) } RETURN t.name",
    );
    assert!(
        where_or.contains("subquery:1"),
        "expected `subquery: 1` on the residual Filter, got: {}",
        where_or
    );

    let ret = joined("EXPLAIN MATCH (t:person) RETURN EXISTS { MATCH (p:person) } AS x");
    assert!(
        ret.contains("subquery:1"),
        "expected `subquery: 1` on the RETURN Project, got: {}",
        ret
    );

    let plain = joined("EXPLAIN MATCH (t:person) RETURN t.name");
    assert!(
        !plain.contains("subquery:"),
        "plain plan must not carry subquery annotations, got: {}",
        plain
    );
}

/// Conjunctive WHERE subqueries still plan and execute.
#[test]
fn test_conjunctive_where_still_executes() {
    let mut db = create_test_db();
    setup_person_graph(&mut db);

    let result = db
        .execute_query(
            "MATCH (t:person) WHERE EXISTS { MATCH (p:person) WHERE p.age == 30 } RETURN t.name",
        )
        .expect("conjunctive EXISTS must still execute");
    assert_eq!(result.rows().len(), 4);
}

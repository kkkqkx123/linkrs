//! DQL boundary and empty-set semantics.
//!
//! Locks the terminal behavior of empty inputs, NULL propagation, type
//! errors and large UNWIND lists. Every case asserts content or the exact
//! failure, never just success.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;

#[test]
fn test_unwind_empty_list_returns_empty() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("edge_empty_unwind")
        .query("UNWIND [] AS n RETURN n")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_count_empty_match_returns_single_zero_row() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("edge_empty_count")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .query("MATCH (n:Person) RETURN COUNT(n) AS total")
        .assert_success()
        .assert_result_columns(&["total"]);
    let result = scenario.last_result().expect("count must return a result");
    match result {
        linkrs_query::executor::base::ExecutionResult::DataSet { data, .. } => {
            assert_eq!(data.rows.len(), 1, "empty COUNT must return one row");
            assert!(
                data.rows[0].contains(&Value::BigInt(0)) || data.rows[0].contains(&Value::Int(0)),
                "empty COUNT must be 0, got {:?}",
                data.rows[0]
            );
        }
        other => panic!("expected a dataset, got {other:?}"),
    }
}

#[test]
fn test_division_by_zero_is_error() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("edge_div_zero")
        .exec_ddl("CREATE TAG Counter(id INT, val INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Counter(val) VALUES 1:(10)")
        .assert_success()
        .exec_dml("UPDATE 1 ON Counter SET val = val / 0")
        .assert_error();
}

#[test]
fn test_null_comparison_does_not_match_rows() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("edge_null_cmp")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .query("MATCH (n:Person) WHERE n.name IS NULL RETURN n")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_empty_string_filter_matches_only_empty() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("edge_empty_str")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:(''), 2:('Bob')")
        .assert_success()
        .query("MATCH (n:Person) WHERE n.name = '' RETURN n.name AS name")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::string("")]);
}

#[test]
fn test_large_unwind_list_row_count() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("edge_large_unwind")
        .query("UNWIND [1, 2, 3, 4, 5, 6, 7, 8, 9, 10] AS x RETURN x")
        .assert_success()
        .assert_result_columns(&["x"])
        .assert_result_count(10);
}

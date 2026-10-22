//! Terminal-state contract for statements that previously had no planner.
//!
//! Each test asserts the user-visible outcome: rows for implemented
//! statements, a readable rejection for statements that moved outside SQL.
//! A `No planner for bound statement` leak fails every case here.

mod common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use std::fs;

fn temp_csv(name: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "linkrs_deadend_{}_{}.csv",
        name,
        std::process::id()
    ));
    fs::write(&path, content).expect("write csv fixture");
    path
}

// ==================== CLEAR SPACE ====================

#[test]
fn test_clear_space_current_space_reports_counts_and_clears_data() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_clear_current")
        .exec_ddl("CREATE TAG person(id INT, name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE knows(weight INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\"), 2:(\"Bob\")")
        .assert_success()
        .exec_dml("INSERT EDGE knows(weight) VALUES 1->2:(1)")
        .assert_success()
        .query("CLEAR SPACE")
        .assert_success()
        .assert_result_count(1)
        .assert_result_columns(&[
            "space_name",
            "cleared_tags",
            "cleared_edge_types",
            "deleted_vertices",
            "deleted_edges",
        ])
        .assert_result_contains(vec![
            Value::string("deadend_clear_current"),
            Value::BigInt(1),
            Value::BigInt(1),
            Value::BigInt(2),
            Value::BigInt(1),
        ])
        .query("MATCH (n) RETURN n")
        .assert_success()
        .assert_result_empty();

    assert!(scenario.error().is_none(), "{:?}", scenario.error());
}

#[test]
fn test_clear_space_named_space() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_clear_named")
        .exec_ddl("CREATE TAG node(id INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX node(id) VALUES 7:(7)")
        .assert_success()
        .query("CLEAR SPACE deadend_clear_named")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::string("deadend_clear_named")])
        .query("MATCH (n) RETURN n")
        .assert_success()
        .assert_result_empty();
}

#[test]
fn test_clear_space_missing_space_is_error() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_clear_present")
        .query("CLEAR SPACE deadend_clear_absent")
        .assert_error()
        .assert_error_contains("does not exist");
}

// ==================== LOAD FROM ====================

#[test]
fn test_load_from_csv_file_reports_rows_and_columns() {
    let path = temp_csv("file", "id,name\n1,Alice\n2,Bob\n");
    let query = format!("LOAD FROM '{}'", path.display());

    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_load_file")
        .query(&query)
        .assert_success()
        .assert_result_count(2)
        .assert_result_columns(&["id", "name"])
        .assert_result_contains(vec![Value::string("Alice")]);

    let _ = fs::remove_file(path);
}

#[test]
fn test_load_from_headers_rename_columns_and_keep_strings() {
    let path = temp_csv("headers", "00123,Alice\n00456,Bob\n");
    let query = format!(
        "LOAD WITH HEADERS (id, name) FROM '{}' (header='false')",
        path.display()
    );

    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_load_headers")
        .query(&query)
        .assert_success()
        .assert_result_count(2)
        .assert_result_columns(&["id", "name"])
        // CSV cells stay strings: `00123` must not become the number 123.
        .assert_result_contains(vec![Value::string("00123"), Value::string("Alice")]);

    let _ = fs::remove_file(path);
}

#[test]
fn test_load_from_missing_file_is_error() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_load_missing")
        .query("LOAD FROM '/nonexistent/linkrs/deadend.csv'")
        .assert_error()
        .assert_error_contains("failed to open");
}

// ==================== CALL ====================

#[test]
fn test_call_builtin_returns_one_row() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_call_builtin")
        .query("CALL db_version()")
        .assert_success()
        .assert_result_count(1)
        .assert_result_columns(&["result"]);
}

#[test]
fn test_call_yield_renames_column() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_call_yield")
        .query("CALL db_version() YIELD version")
        .assert_success()
        .assert_result_count(1)
        .assert_result_columns(&["version"]);

    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_call_yield_alias")
        .query("CALL db_version() YIELD version AS v")
        .assert_success()
        .assert_result_columns(&["v"]);
}

#[test]
fn test_call_unknown_function_propagates_error() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_call_unknown")
        .query("CALL not_a_real_function()")
        .assert_error()
        .assert_error_contains("CALL not_a_real_function failed");
}

#[test]
fn test_call_rejects_non_literal_argument() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_call_expr")
        .query("CALL db_version(1 + 2)")
        .assert_error()
        .assert_error_contains("arguments must be literals");
}

// ==================== Explicitly rejected statements ====================

#[test]
fn test_extension_sql_points_to_extension_apis() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_extension")
        .query("INSTALL EXTENSION my_udf FROM '/tmp/udf_plugin.so'")
        .assert_error()
        .assert_error_contains("Extension lifecycle is managed outside SQL")
        .assert_error_contains("/v1/functions");
}

#[test]
fn test_migrate_sql_points_to_migration_apis() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_migrate")
        .query("MIGRATE PLAN FOR TAG person FROM VERSION 1 TO 2 IN deadend_migrate")
        .assert_error()
        .assert_error_contains("MIGRATE is not available through SQL")
        .assert_error_contains("/v1/migration/plan");
}

#[test]
fn test_query_statement_is_gone() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("deadend_query")
        .query("QUERY MATCH (n) RETURN n")
        .assert_error();
}

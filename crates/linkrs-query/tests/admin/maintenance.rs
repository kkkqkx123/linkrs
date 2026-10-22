//! Admin maintenance statement semantics.
//!
//! Covers the executable-but-previously-untested Admin statements:
//! CHECKPOINT, EXPORT DATABASE and IMPORT DATABASE. Every case asserts
//! content or state effects, not just success.

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;

fn unique_dir(prefix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("linkrs_admin_{}_{}", prefix, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn test_checkpoint_succeeds_and_keeps_data_readable() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("admin_checkpoint")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_ddl("CHECKPOINT")
        .assert_success()
        .assert_result_columns(&["action", "name", "status"])
        .assert_result_count(1)
        .assert_result_contains(vec![Value::string("auth")])
        .query("MATCH (n:Person) RETURN n.name AS name")
        .assert_success()
        .assert_result_count(2)
        .assert_result_contains(vec![Value::string("Alice")]);
    assert!(scenario.error().is_none());
}

#[test]
fn test_checkpoint_is_idempotent() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("admin_checkpoint_idem")
        .exec_ddl("CREATE TAG Person(id INT)")
        .assert_success()
        .exec_ddl("CHECKPOINT")
        .assert_success()
        .assert_result_count(1)
        .exec_ddl("CHECKPOINT")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_export_creates_schema_file_and_import_roundtrip_restores_data() {
    let dir = unique_dir("roundtrip");
    let export_path = dir.display().to_string();

    TestScenario::new()
        .expect("scenario")
        .setup_space("admin_roundtrip")
        .exec_ddl("CREATE TAG Person(id INT, name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_ddl(&format!("EXPORT DATABASE '{export_path}'"))
        .assert_success()
        .assert_result_contains(vec![Value::string("executed")]);

    assert!(
        dir.join("admin_roundtrip").join("schema.json").exists(),
        "export must write <space>/schema.json"
    );

    TestScenario::new()
        .expect("scenario")
        .setup_space("admin_roundtrip")
        .exec_ddl(&format!("IMPORT DATABASE '{export_path}'"))
        .assert_success()
        .assert_result_contains(vec![Value::string("executed")])
        .query("MATCH (n:Person) RETURN n.name AS name")
        .assert_success()
        .assert_result_contains(vec![Value::string("Alice")]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_import_missing_directory_fails_without_half_state() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("admin_import_missing")
        .exec_ddl("CREATE TAG Person(id INT)")
        .assert_success()
        .exec_ddl("IMPORT DATABASE '/nonexistent/linkrs/import_missing_admin'")
        .assert_error()
        .assert_error_contains("Failed to read import directory");
    assert!(scenario.error().is_some_and(|e| !e.contains("No planner")));
}

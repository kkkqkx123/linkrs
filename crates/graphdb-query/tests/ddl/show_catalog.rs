//! SHOW Catalog Integration Tests
//!
//! Test coverage:
//! - SHOW ATTACHED DATABASES returns an empty list when nothing is attached
//! - SHOW EXTENSIONS returns an empty list when no dynamic UDF is loaded

use super::common;

use common::test_scenario::TestScenario;

/// SHOW ATTACHED DATABASES with an empty catalog returns zero rows.
#[test]
fn test_show_attached_databases_empty() {
    graphdb_query::attached::clear_attached_databases();
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("SHOW ATTACHED DATABASES")
        .assert_result_count(0);
}

/// SHOW EXTENSIONS with no loaded UDF returns zero rows.
#[test]
fn test_show_extensions_empty() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("SHOW EXTENSIONS")
        .assert_result_count(0);
}

/// Attach records appear in SHOW ATTACHED DATABASES output.
#[test]
fn test_show_attached_databases_lists_attachments() {
    use graphdb_core::Value;
    use graphdb_query::attached::{attach_database, AttachedDatabase};

    graphdb_query::attached::clear_attached_databases();
    attach_database(AttachedDatabase::new(
        "analytics".to_string(),
        "/tmp/analytics".to_string(),
        Some("KUZU".to_string()),
    ))
    .expect("attach succeeds");
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .query("SHOW ATTACHED DATABASES")
        .assert_result_contains(vec![Value::string("analytics")]);
    graphdb_query::attached::clear_attached_databases();
}

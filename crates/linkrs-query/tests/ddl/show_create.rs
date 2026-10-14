//! SHOW CREATE Output Contract Tests (Phase 2 wiring)
//!
//! Test coverage:
//! - SHOW CREATE TAG/EDGE/SPACE/INDEX all return a single `create_statement` column
//! - TAG rendering carries NOT NULL / DEFAULT / COMMENT and re-parses
//! - EDGE rendering carries FROM ... TO ... endpoint constraints
//! - SPACE rendering carries vid_type and comment
//! - INDEX rendering carries ON <target>(<props>)

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::Parser;

fn ddl_text(scenario: &TestScenario) -> String {
    match scenario.last_result().expect("statement returns rows") {
        ExecutionResult::DataSet { data: ds, .. } => {
            assert_eq!(ds.rows.len(), 1, "expected exactly one DDL row");
            match &ds.rows[0][0] {
                Value::String(ddl) => ddl.to_string(),
                other => panic!("expected string DDL, got {other:?}"),
            }
        }
        other => panic!("expected dataset, got {other:?}"),
    }
}

fn assert_reparseable(ddl: &str) {
    let mut parser = Parser::new(ddl);
    parser.parse().unwrap_or_else(|e| {
        panic!("SHOW CREATE output must re-parse, got {e:?} for: {ddl}");
    });
}

#[test]
fn test_show_create_tag_full_constraints() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            "CREATE TAG Person(id: INT NOT NULL COMMENT 'pk', \
             name: STRING NOT NULL DEFAULT 'unknown' COMMENT 'nm', \
             age: INT DEFAULT 0)",
        )
        .assert_success()
        .query("SHOW CREATE TAG Person")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(ddl.contains("NOT NULL"), "DDL must carry NOT NULL, got: {ddl}");
    assert!(
        ddl.contains("DEFAULT 'unknown'"),
        "DDL must carry string default as SQL literal, got: {ddl}"
    );
    assert!(
        ddl.contains("DEFAULT 0"),
        "DDL must carry numeric default, got: {ddl}"
    );
    assert!(
        ddl.contains("COMMENT 'pk'"),
        "DDL must carry comments, got: {ddl}"
    );
    assert_reparseable(&ddl);
}

#[test]
fn test_show_create_tag_serial_round_trip() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Counter(name: STRING, seq: SERIAL)")
        .assert_success()
        .query("SHOW CREATE TAG Counter")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(ddl.contains("SERIAL"), "DDL must keep SERIAL, got: {ddl}");
    assert_reparseable(&ddl);
}

#[test]
fn test_show_create_edge_with_endpoints() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE TAG Company(name: STRING)")
        .exec_ddl(
            "CREATE EDGE WORKS_AT(since: DATE NOT NULL COMMENT 'd', \
             department: STRING DEFAULT 'unknown') FROM Person TO Company",
        )
        .assert_success()
        .query("SHOW CREATE EDGE WORKS_AT")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(
        ddl.contains("FROM Person TO Company"),
        "DDL must carry endpoint constraints, got: {ddl}"
    );
    assert!(ddl.contains("NOT NULL"), "DDL must carry NOT NULL, got: {ddl}");
    assert!(
        ddl.contains("DEFAULT 'unknown'"),
        "DDL must carry defaults, got: {ddl}"
    );
    assert_reparseable(&ddl);
}

#[test]
fn test_show_create_space_unified_column() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE idx_space(vid_type=INT64, comment='s')")
        .assert_success()
        .query("SHOW CREATE SPACE idx_space")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(ddl.contains("vid_type=INT64"), "got: {ddl}");
    assert!(ddl.contains("comment='s'"), "got: {ddl}");
}

#[test]
fn test_show_create_tag_index() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .exec_ddl("CREATE TAG INDEX idx_person_name ON Person(name)")
        .assert_success()
        .query("SHOW CREATE INDEX idx_person_name")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(
        ddl.contains("ON Person(name)"),
        "DDL must carry index target and fields, got: {ddl}"
    );
}

#[test]
fn test_show_create_edge_index() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since: STRING) FROM Person TO Person")
        .exec_ddl("CREATE EDGE INDEX idx_knows_since ON KNOWS(since)")
        .assert_success()
        .query("SHOW CREATE INDEX idx_knows_since")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    let ddl = ddl_text(&scenario);
    assert!(
        ddl.contains("ON KNOWS(since)"),
        "DDL must carry edge index target and fields, got: {ddl}"
    );
}

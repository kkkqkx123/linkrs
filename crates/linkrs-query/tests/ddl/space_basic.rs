//! DDL Space Basic Tests
//!
//! Test coverage:
//! - CREATE SPACE - Create graph space
//! - DROP SPACE - Delete graph space, with IF EXISTS
//! - DROP GRAPH - Namespace alias for DROP SPACE

use super::common;

use common::test_scenario::TestScenario;

// ==================== DROP SPACE Execution Tests ====================

#[test]
fn test_drop_space_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SPACE IF EXISTS missing_space")
        .assert_success();
}

#[test]
fn test_drop_space_execution_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP SPACE missing_space")
        .assert_error();
}

// ==================== DROP GRAPH Execution Tests ====================

#[test]
fn test_drop_graph_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("graph_space")
        .exec_ddl("DROP GRAPH graph_space")
        .assert_success()
        .exec_ddl("DROP SPACE graph_space")
        .assert_error();
}

#[test]
fn test_drop_graph_execution_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP GRAPH no_such_space")
        .assert_error();
}

#[test]
fn test_drop_graph_execution_if_exists_missing() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP GRAPH IF EXISTS no_such_space")
        .assert_success();
}

// ==================== CREATE SPACE comment / vid_type Wiring Tests ====================

#[test]
fn test_create_space_comment_and_bare_fixedstring() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE cspace(vid_type=FIXEDSTRING32, comment='x')")
        .assert_success()
        .query("DESC SPACE cspace")
        .assert_success()
        .assert_result_contains(vec![
            linkrs_core::Value::string("FixedString(32)"),
            linkrs_core::Value::string("x"),
        ]);
}

#[test]
fn test_create_space_with_comment_clause() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE wspace WITH comment='hello'")
        .assert_success()
        .query("DESC SPACE wspace")
        .assert_success()
        .assert_result_contains(vec![linkrs_core::Value::string("hello")]);
}

#[test]
fn test_create_space_unknown_with_option_is_rejected() {
    use linkrs_query::parser::Parser;

    let query = "CREATE SPACE badspace WITH partition_num=10";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(
        result.is_err(),
        "unknown WITH option should be rejected: {}",
        query
    );
    let collected = format!("{:?}", parser.take_errors());
    assert!(
        collected.contains("Unsupported space option 'partition_num'"),
        "rejection must name the unsupported option for {query}: {collected}"
    );
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl(query)
        .assert_error();
}

#[test]
fn test_create_space_unknown_paren_option_is_rejected() {
    use linkrs_query::parser::Parser;

    let query = "CREATE SPACE badspace2(partition_num=10)";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(
        result.is_err(),
        "unknown paren option should be rejected: {}",
        query
    );
    let collected = format!("{:?}", parser.take_errors());
    assert!(
        collected.contains("Unsupported space option"),
        "rejection must list supported options for {query}: {collected}"
    );
}

#[test]
fn test_create_space_unknown_vid_type_is_rejected() {
    use linkrs_query::parser::Parser;

    let query = "CREATE SPACE badspace3(vid_type=FOO)";
    let mut parser = Parser::new(query);
    let result = parser.parse();
    assert!(result.is_err(), "unknown vid_type should be rejected");
    let collected = format!("{:?}", parser.take_errors());
    assert!(
        collected.contains("Unknown vid_type 'FOO'"),
        "rejection must name the bad vid_type for {query}: {collected}"
    );
}

#[test]
fn test_show_create_space_renders_vid_type_and_comment() {
    use linkrs_query::executor::base::ExecutionResult;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE sc_space(vid_type=FIXED_STRING(16), comment='hi')")
        .assert_success()
        .query("SHOW CREATE SPACE sc_space")
        .assert_success()
        .assert_result_columns(&["create_statement"]);
    match scenario.last_result().expect("SHOW CREATE SPACE returns rows") {
        ExecutionResult::DataSet { data: ds, .. } => {
            assert_eq!(ds.rows.len(), 1, "expected one DDL row");
            match &ds.rows[0][0] {
                linkrs_core::Value::String(ddl) => {
                    assert!(
                        ddl.contains("vid_type=FIXED_STRING(16)"),
                        "DDL must carry vid_type, got: {ddl}"
                    );
                    assert!(
                        ddl.contains("comment='hi'"),
                        "DDL must carry comment, got: {ddl}"
                    );
                }
                other => panic!("expected string DDL, got {other:?}"),
            }
        }
        other => panic!("expected dataset, got {other:?}"),
    }
}

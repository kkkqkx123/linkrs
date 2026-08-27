//! ANALYZE statement integration tests
//!
//! Test Range.
//! - ANALYZE - statistics collection for a space
//! - Idempotency - repeated ANALYZE produces consistent results
//! - DDL invalidation - statistics are invalidated after DDL and refreshed

mod common;

#[test]
fn test_analyze_collects_vertex_and_edge_counts() {
    use common::test_scenario::TestScenario;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("analyze_space")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG company(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE follow(follow_rank INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\"), 2:(\"Bob\"), 3:(\"Carol\")")
        .assert_success()
        .exec_dml("INSERT VERTEX company(name) VALUES 10:(\"ACME\")")
        .assert_success()
        .exec_dml("INSERT EDGE follow(follow_rank) VALUES 1->2:(1), 1->3:(1), 2->3:(1)")
        .assert_success()
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 3)
        .assert_analyzed_vertex_count("company", 1)
        .assert_analyzed_edge_count("follow", 3);

    assert!(scenario.error().is_none());
}

#[test]
fn test_analyze_idempotent_and_refreshes_after_data_change() {
    use common::test_scenario::TestScenario;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("analyze_idem_space")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\"), 2:(\"Bob\")")
        .assert_success()
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 2)
        // Second ANALYZE without schema changes yields the same result.
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 2)
        // Data change without DDL: explicit ANALYZE refreshes the counts.
        .exec_dml("INSERT VERTEX person(name) VALUES 3:(\"Carol\"), 4:(\"Dave\")")
        .assert_success()
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 4);

    assert!(scenario.error().is_none());
}

#[test]
fn test_analyze_after_ddl_invalidates_stale_statistics() {
    use common::test_scenario::TestScenario;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("analyze_ddl_space")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\")")
        .assert_success()
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 1);

    let version_before_ddl = scenario
        .stats_manager()
        .space_version("analyze_ddl_space")
        .expect("space version should be recorded after ANALYZE");

    // DDL invalidates statistics: the recorded version is dropped.
    let scenario = scenario
        .exec_ddl("CREATE TAG company(name STRING)")
        .assert_success()
        .analyze()
        .assert_success()
        .assert_analyzed_vertex_count("person", 1)
        .assert_analyzed_vertex_count("company", 0);

    let version_after_ddl = scenario
        .stats_manager()
        .space_version("analyze_ddl_space")
        .expect("space version should be recorded after re-ANALYZE");
    assert_ne!(
        version_before_ddl, version_after_ddl,
        "ANALYZE after DDL should re-collect with a new schema version"
    );

    assert!(scenario.error().is_none());
}

#[test]
fn test_analyze_without_space_errors() {
    use common::test_scenario::TestScenario;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .query("ANALYZE")
        .assert_error();

    assert!(scenario.error().is_some());
}

#[test]
fn test_analyze_parser_variants() {
    use graphdb_query::parser::Parser;

    let mut parser = Parser::new("ANALYZE SPACE basketball");
    let result = parser.parse();
    assert!(
        result.is_ok(),
        "ANALYZE SPACE parse failed: {:?}",
        result.err()
    );
    let stmt = result.expect("ANALYZE SPACE should parse");
    assert_eq!(stmt.ast.stmt.kind(), "ANALYZE");
}

//! Full-text statement contract.
//!
//! Two layers, matching the feature gate:
//! - feature-independent: without `fulltext`, every full-text statement must
//!   be rejected with a readable "not enabled" message, never the internal
//!   planner leak.
//! - feature-gated: with `fulltext`, index creation, search, lookup and the
//!   index-management statements execute with content assertions.

use super::common;

use common::test_scenario::TestScenario;

const LEAK: &str = "No planner for bound statement";

const FULLTEXT_SQLS: &[(&str, &str)] = &[
    (
        "CREATE FULLTEXT INDEX idx_article_content ON article(content) ENGINE BM25",
        "CREATE FULLTEXT INDEX",
    ),
    (
        "DROP FULLTEXT INDEX idx_article_content",
        "DROP FULLTEXT INDEX",
    ),
    (
        "ALTER FULLTEXT INDEX idx_article_content REBUILD",
        "ALTER FULLTEXT INDEX",
    ),
    ("SHOW FULLTEXT INDEX", "SHOW FULLTEXT INDEX"),
    (
        "DESCRIBE FULLTEXT INDEX idx_article_content",
        "DESCRIBE FULLTEXT INDEX",
    ),
    (
        "SEARCH INDEX idx_article_content MATCH 'database'",
        "SEARCH",
    ),
    (
        "LOOKUP ON article INDEX idx_article_content WHERE 'database'",
        "LOOKUP FULLTEXT",
    ),
];

#[cfg(not(feature = "fulltext"))]
#[test]
fn test_fulltext_statements_rejected_without_feature() {
    for (sql, label) in FULLTEXT_SQLS {
        let scenario = TestScenario::new()
            .expect("scenario")
            .setup_space("fulltext_off")
            .exec_ddl("CREATE TAG article(title STRING, content STRING)")
            .assert_success()
            .query(sql)
            .assert_error();
        let error = scenario.error().expect("fulltext statement must fail");
        assert!(
            !error.contains(LEAK),
            "{label} leaks the internal planner error: {error}"
        );
        assert!(
            error.contains("not enabled"),
            "{label} must name the disabled feature, got: {error}"
        );
    }
}

#[cfg(not(feature = "fulltext"))]
#[test]
fn test_match_fulltext_rejected_without_feature() {
    let scenario = TestScenario::new()
        .expect("scenario")
        .setup_space("fulltext_off_match")
        .exec_ddl("CREATE TAG article(title STRING, content STRING)")
        .assert_success()
        .query("MATCH \"(n:article)\" WHERE FULLTEXT_MATCH(content, 'database')")
        .assert_error();
    let error = scenario.error().expect("MATCH FULLTEXT must fail");
    assert!(!error.contains(LEAK), "leak: {error}");
    assert!(
        error.contains("not enabled"),
        "must name the disabled feature, got: {error}"
    );
}

#[cfg(feature = "fulltext")]
#[test]
fn test_fulltext_create_search_lookup_roundtrip() {
    use linkrs_core::Value;
    TestScenario::new()
        .expect("scenario")
        .setup_space("fulltext_on")
        .exec_ddl("CREATE TAG article(title STRING, content STRING)")
        .assert_success()
        .exec_ddl("CREATE FULLTEXT INDEX idx_article_content ON article(content) ENGINE BM25")
        .assert_success()
        .exec_dml("INSERT VERTEX article(title, content) VALUES 1:('a', 'database index engine'), 2:('b', 'unrelated words here')")
        .assert_success()
        .query("SEARCH INDEX idx_article_content MATCH 'database'")
        .assert_success()
        .query("SHOW FULLTEXT INDEX")
        .assert_success()
        .assert_result_contains(vec![Value::string("idx_article_content")])
        .query("DESCRIBE FULLTEXT INDEX idx_article_content")
        .assert_success()
        .assert_result_contains(vec![Value::string("idx_article_content")])
        .query("DROP FULLTEXT INDEX idx_article_content")
        .assert_success();
}

#[cfg(feature = "fulltext")]
#[test]
fn test_fulltext_rebuild_points_to_rebuild_api() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("fulltext_rebuild")
        .exec_ddl("CREATE TAG article(title STRING, content STRING)")
        .assert_success()
        .exec_ddl("CREATE FULLTEXT INDEX idx_article_content ON article(content) ENGINE BM25")
        .assert_success()
        .query("ALTER FULLTEXT INDEX idx_article_content REBUILD")
        .assert_error()
        .assert_error_contains("/v1/fulltext/indexes/rebuild");
}

#[cfg(feature = "fulltext")]
#[test]
fn test_fulltext_search_unknown_index_is_error_not_empty() {
    TestScenario::new()
        .expect("scenario")
        .setup_space("fulltext_unknown")
        .query("SEARCH INDEX idx_missing MATCH 'database'")
        .assert_error();
}

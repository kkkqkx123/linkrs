//! DDL contract-gap execution tests
//!
//! These tests lock behavior that was previously covered only by parser smoke tests:
//! unsupported (but syntactically close) inputs must fail explicitly, and normal
//! spaces/DDL objects must remain usable after the failed statements.

use super::common;

use common::test_scenario::TestScenario;

#[test]
fn test_create_space_unknown_option_errors() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE option_space WITH partition_num=10");

    let err = scenario
        .error()
        .expect("unexpected successful CREATE SPACE WITH unknown option");
    assert!(
        err.contains("Unsupported space option"),
        "error must identify unsupported options, got: {err}"
    );

    // The failed CREATE must not block a valid space creation.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE fallback_space")
        .assert_success();
}

#[test]
fn test_create_tag_as_query_generic_path_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG adult AS (MATCH (p:Person) RETURN p)");

    let err = scenario
        .error()
        .expect("generic CREATE TAG AS query unexpectedly succeeded");
    assert!(
        err.contains("is only supported through the session API"),
        "error must state the supported execution path, got: {err}"
    );
}

#[test]
fn test_create_edge_as_query_generic_path_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE EDGE adult_edge AS (MATCH (a:Person)->(b:Company) RETURN a, b)");

    let err = scenario
        .error()
        .expect("generic CREATE EDGE AS query unexpectedly succeeded");
    assert!(
        err.contains("is only supported through the session API"),
        "error must state the supported execution path, got: {err}"
    );
}

#[cfg(feature = "fulltext")]
#[test]
fn test_create_fulltext_unsupported_analyzer_is_parse_error() {
    use linkrs_query::parser::Parser;

    let result = Parser::new(
        "CREATE FULLTEXT INDEX idx ON Article(content ANALYZER 'standard') ENGINE = BM25",
    )
    .parse();

    let err = result.expect_err("ANALYZER clause must be rejected");
    assert!(
        format!("{err:?}").contains("ANALYZER is not supported"),
        "parse error must name unsupported ANALYZER clause, got: {err:?}"
    );
}

#[cfg(feature = "fulltext")]
#[test]
fn test_create_fulltext_missing_engine_is_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Article(title: STRING)")
        .assert_success()
        .exec_ddl("CREATE FULLTEXT INDEX idx ON Article(title)");

    assert!(
        scenario.error().is_some(),
        "CREATE FULLTEXT INDEX without ENGINE = BM25 must fail"
    );
}

#[cfg(feature = "vector")]
#[test]
fn test_create_vector_invalid_hnsw_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Doc(name: STRING)")
        .assert_success()
        .exec_ddl(
            "CREATE VECTOR INDEX idx ON Doc(embedding) \
             WITH (vector_size=4, hnsw_m=1)",
        );

    let err = scenario
        .error()
        .expect("invalid HNSW parameter unexpectedly succeeded");
    assert!(
        err.contains("invalid HNSW parameters") || err.contains("HNSW"),
        "error must name invalid HNSW parameters, got: {err}"
    );
}

#[cfg(feature = "vector")]
#[test]
fn test_create_vector_compression_wrong_quantization_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Doc(name: STRING)")
        .assert_success()
        .exec_ddl(
            "CREATE VECTOR INDEX idx ON Doc(embedding) \
             WITH (vector_size=4, quantization=SCALAR, compression=x16)",
        );

    let err = scenario
        .error()
        .expect("compression on scalar quantization unexpectedly succeeded");
    assert!(
        err.contains("compression requires quantization=PRODUCT"),
        "error must identify unsupported compression/quantization combination, got: {err}"
    );
}

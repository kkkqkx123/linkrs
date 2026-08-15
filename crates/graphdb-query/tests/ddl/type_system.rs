//! Type System Integrity DDL Tests
//!
//! Test coverage:
//! - `TIMESTAMP` keyword normalizes to `DATETIME` (orphan type removal)
//! - `vid_type=VID` is rejected with an explicit error
//! - Valid `vid_type` values (INT64 / STRING / FIXED_STRING) are accepted

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;

// ==================== TIMESTAMP -> DATETIME Normalization ====================

/// TC-TS-NORM-001: `CREATE TAG` with a `TIMESTAMP` column must normalize the
/// column type to `DATETIME` (visible through `DESC TAG`).
#[test]
fn test_create_tag_timestamp_normalizes_to_datetime() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Event(name STRING, created_at TIMESTAMP)")
        .assert_success()
        .query("DESC TAG Event")
        .assert_result_contains(vec![Value::string("DATETIME")]);

    let plan = scenario
        .get_plan_string()
        .expect("DESC TAG should return a dataset");
    assert!(
        !plan.contains("TIMESTAMP") && !plan.contains("Timestamp"),
        "TIMESTAMP must not survive as an orphan type, plan: {plan}"
    );
}

/// TC-TS-NORM-002: `TIMESTAMP` with a `DEFAULT` value works after the
/// normalization (the stored column is `DATETIME`).
#[test]
fn test_create_tag_timestamp_with_default() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"CREATE TAG Event(name STRING, created_at TIMESTAMP DEFAULT "2024-01-01 00:00:00")"#,
        )
        .assert_success()
        .exec_dml("INSERT VERTEX Event(name) VALUES 1:('alice')")
        .assert_success()
        .assert_vertex_count("Event", 1);
}

// ==================== vid_type Validation ====================

/// TC-VID-001: `CREATE SPACE ... (vid_type=VID)` must fail with an explicit
/// error instead of creating a corrupt 0-byte fixed column.
#[test]
fn test_create_space_vid_type_vid_is_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE rejected_space (vid_type=VID)");

    let error = scenario
        .error()
        .expect("CREATE SPACE with vid_type=VID must fail");
    assert!(
        error.contains("VID is not a valid vertex ID type"),
        "error must guide the user to a real type, got: {error}"
    );
}

/// TC-VID-002: Valid integer vid_type is accepted and reported via `DESC SPACE`.
#[test]
fn test_create_space_vid_type_int64_is_accepted() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE int_space (vid_type=INT64)")
        .assert_success()
        .query("DESC SPACE int_space")
        .assert_result_contains(vec![Value::string("BigInt")]);
}

/// TC-VID-003: String vid_type is accepted and reported via `DESC SPACE`.
#[test]
fn test_create_space_vid_type_string_is_accepted() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE str_space (vid_type=STRING)")
        .assert_success()
        .query("DESC SPACE str_space")
        .assert_result_contains(vec![Value::string("String")]);
}

/// TC-VID-004: The default vid_type (no option) is INT64.
#[test]
fn test_create_space_default_vid_type_is_bigint() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE default_space")
        .assert_success()
        .query("DESC SPACE default_space")
        .assert_result_contains(vec![Value::string("BigInt")]);
}

/// TC-VID-005: FIXED_STRING vid_type is accepted (documented alternative to VID).
#[test]
fn test_create_space_vid_type_fixed_string_is_accepted() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("CREATE SPACE fs_space (vid_type=FIXED_STRING(32))")
        .assert_success()
        .query("DESC SPACE fs_space")
        .assert_result_contains(vec![Value::string("FixedString(32)")]);
}

// ==================== Unified Type Name Parsing ====================

/// TC-TY-001: Types previously rejected as "Unknown data type" in DDL (they
/// were accepted by the HTTP/CAST parsers) are now supported through the core
/// `DataType::from_str` source of truth.
#[test]
fn test_create_tag_new_scalar_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"CREATE TAG Types(
                a SMALLINT, b BIGINT, c TIME, d UUID,
                e JSON, f JSONB, g BLOB, h INTERVAL, i DATASET)"#,
        )
        .assert_success()
        .query("DESC TAG Types")
        .assert_result_contains(vec![Value::string("SMALLINT")])
        .assert_result_contains(vec![Value::string("BIGINT")])
        .assert_result_contains(vec![Value::string("TIME")])
        .assert_result_contains(vec![Value::string("UUID")])
        .assert_result_contains(vec![Value::string("JSON")])
        .assert_result_contains(vec![Value::string("JSONB")])
        .assert_result_contains(vec![Value::string("BLOB")])
        .assert_result_contains(vec![Value::string("INTERVAL")])
        .assert_result_contains(vec![Value::string("DATASET")]);
}

/// TC-TY-002: `INT8` maps to `BIGINT` in DDL (PostgreSQL convention by bit
/// width), consistent with the CAST and HTTP parsers.
#[test]
fn test_create_tag_int8_maps_to_bigint() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IntAlias(v INT8)")
        .assert_success()
        .query("DESC TAG IntAlias")
        .assert_result_contains(vec![Value::string("BIGINT")]);
}

/// TC-TY-003: `INT16`/`INT32` map to `INT`, `INT8`/`INT64` to `BIGINT`,
/// `INT2` to `SMALLINT`.
#[test]
fn test_create_tag_int_width_aliases() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"CREATE TAG IntWidth(a INT16, b INT32, c INT64, d INT2, e INT4)"#,
        )
        .assert_success()
        .query("DESC TAG IntWidth")
        .assert_result_contains(vec![Value::string("SMALLINT")])
        .assert_result_contains(vec![Value::string("BIGINT")])
        .assert_result_contains(vec![Value::string("INT")]);
}

/// TC-TY-004: `TIMESTAMP` remains normalized to `DATETIME`; the unified
/// parser keeps the established DDL/HTTP/CAST semantics.
#[test]
fn test_timestamp_keyword_still_normalizes_to_datetime() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Ts(v TIMESTAMP)")
        .assert_success()
        .query("DESC TAG Ts")
        .assert_result_contains(vec![Value::string("DATETIME")]);
}

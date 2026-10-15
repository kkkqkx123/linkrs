//! DDL Tag Basic Tests
//!
//! Test coverage:
//! - CREATE TAG - Create vertex tag (including TTL expiry configuration)
//! - DROP TAG - Delete vertex tag (single, batch, IF EXISTS)
//! - DESC TAG - Describe tag schema
//! - TTL - Expiry filtering and physical purge of expired vertices

use super::common;

use common::test_scenario::TestScenario;
use linkrs_core::Value;
use linkrs_query::parser::Parser;
use std::collections::HashMap;

// ==================== CREATE TAG Parser Tests ====================

#[test]
fn test_create_tag_parser_basic() {
    let query = "CREATE TAG Person(name: STRING, age: INT)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_parser_with_if_not_exists() {
    let query = "CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with IF NOT EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_parser_single_property() {
    let query = "CREATE TAG Person(name: STRING)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG single property parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_parser_multiple_properties() {
    let query = "CREATE TAG Person(name: STRING, age: INT, created_at: TIMESTAMP)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG multiple properties parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_tag_parser_various_types() {
    let query = "CREATE TAG Test(name: STRING, age: INT, score: DOUBLE, active: BOOL, birth: DATE)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG various types parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

// ==================== CREATE TAG Execution Tests ====================

#[test]
fn test_create_tag_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person");
}

#[test]
fn test_create_tag_execution_with_if_not_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(id: INT, name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(id: INT, name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person");
}

#[test]
fn test_create_tag_execution_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([("name", Value::string("Alice")), ("age", Value::Int(30))]),
        );
}

// ==================== DROP TAG Parser Tests ====================

#[test]
fn test_drop_tag_parser_basic() {
    let query = "DROP TAG Person";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP TAG basic parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_tag_parser_with_if_exists() {
    let query = "DROP TAG IF EXISTS Person";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP TAG with IF EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_tag_parser_multiple() {
    let query = "DROP TAG Person, Company, Location";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP TAG multiple tags parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

#[test]
fn test_drop_tag_parser_multiple_with_if_exists() {
    let query = "DROP TAG IF EXISTS Person, Company";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DROP TAG multiple tags with IF EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DROP TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DROP");
}

// ==================== DROP TAG Execution Tests ====================

#[test]
fn test_drop_tag_execution_basic() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_ddl("DROP TAG Person")
        .assert_success()
        .assert_tag_not_exists("Person");
}

#[test]
fn test_drop_tag_execution_with_if_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP TAG IF EXISTS NonExistentTag")
        .assert_success()
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING)")
        .assert_success()
        .exec_ddl("DROP TAG IF EXISTS Person")
        .assert_success()
        .assert_tag_not_exists("Person");
}

#[test]
fn test_drop_tag_execution_multiple() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG BatchA(id: INT)")
        .assert_success()
        .exec_ddl("CREATE TAG BatchB(id: INT)")
        .assert_success()
        .exec_ddl("CREATE TAG BatchC(id: INT)")
        .assert_success()
        .exec_ddl("DROP TAG BatchA, BatchB, BatchC")
        .assert_success()
        .assert_tag_not_exists("BatchA")
        .assert_tag_not_exists("BatchB")
        .assert_tag_not_exists("BatchC");
}

#[test]
fn test_drop_tag_execution_multiple_with_if_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG BatchX(id: INT)")
        .assert_success()
        .exec_ddl("DROP TAG IF EXISTS BatchX, BatchMissing")
        .assert_success()
        .assert_tag_not_exists("BatchX");
}

#[test]
fn test_drop_tag_execution_multiple_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG BatchY(id: INT)")
        .assert_success()
        .exec_ddl("DROP TAG BatchY, BatchMissing")
        .assert_error();
}

#[test]
fn test_drop_tag_execution_missing_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("DROP TAG no_such_tag")
        .assert_error();
}

// ==================== DESC TAG Tests ====================

#[test]
fn test_desc_parser_tag() {
    let query = "DESCRIBE TAG Person";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DESCRIBE TAG parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DESCRIBE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DESC");
}

#[test]
fn test_desc_parser_short_tag() {
    let query = "DESC TAG Person";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DESC TAG parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DESC TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DESC");
}

#[test]
fn test_desc_execution_tag() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING, age: INT)")
        .assert_success()
        .query("DESCRIBE TAG Person")
        .assert_success()
        .assert_result_count(3)
        .assert_result_contains(vec![Value::string("name"), Value::string("STRING")])
        .assert_result_contains(vec![Value::string("age"), Value::string("INT")]);
}

#[test]
fn test_desc_execution_tag_with_constraints() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id: INT, name: STRING NOT NULL, age: INT DEFAULT 0)")
        .assert_success()
        .query("DESCRIBE TAG Person")
        .assert_success()
        .assert_result_count(3);
}

// ==================== Tag Lifecycle Tests ====================

#[test]
fn test_ddl_tag_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG TestTag(id: INT, name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("TestTag")
        .query("DESCRIBE TAG TestTag")
        .assert_success()
        .exec_ddl("ALTER TAG TestTag ADD (email: STRING)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX TestTag(name, age, email) VALUES 1:('Alice', 30, 'alice@test.com')",
        )
        .assert_success()
        .assert_vertex_exists(1, "TestTag")
        .exec_ddl("ALTER TAG TestTag DROP (email)")
        .assert_success()
        .exec_dml("DELETE VERTEX TestTag FROM 1")
        .assert_success()
        .exec_ddl("DROP TAG TestTag")
        .assert_success()
        .assert_tag_not_exists("TestTag");
}

#[test]
fn test_ddl_if_not_exists_if_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(id: INT, name: STRING)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(id: INT, name: STRING)")
        .assert_success()
        .exec_ddl("DROP TAG IF EXISTS Person")
        .assert_success()
        .assert_tag_not_exists("Person")
        .exec_ddl("DROP TAG IF EXISTS Person")
        .assert_success();
}

// ==================== GEOGRAPHY Type Tests ====================

/// TC-GEO-TYPE-001: Parse CREATE TAG with GEOGRAPHY type (keyword)
#[test]
fn test_create_tag_parser_geography_keyword() {
    let query = "CREATE TAG Location(name: STRING, coord: GEOGRAPHY)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with GEOGRAPHY type parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-GEO-TYPE-002: Parse CREATE TAG with multiple GEOGRAPHY fields
#[test]
fn test_create_tag_parser_multiple_geography_fields() {
    let query = "CREATE TAG City(name: STRING, center: GEOGRAPHY, boundary: GEOGRAPHY)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with multiple GEOGRAPHY fields parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-GEO-TYPE-003: Execute CREATE TAG with GEOGRAPHY type
#[test]
fn test_create_tag_execution_geography() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Location(id: INT, name: STRING, coord: GEOGRAPHY)")
        .assert_success()
        .assert_tag_exists("Location");
}

/// TC-GEO-TYPE-004: Execute CREATE TAG with GEOGRAPHY and insert data
#[test]
fn test_create_tag_geography_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG City(id: INT, name: STRING, center: GEOGRAPHY)")
        .assert_success()
        .assert_tag_exists("City")
        .exec_dml("INSERT VERTEX City(name) VALUES 1:('Beijing')")
        .assert_success()
        .assert_vertex_exists(1, "City");
}

// ==================== VECTOR Type Tests ====================

/// TC-VEC-TYPE-001: Parse CREATE TAG with VECTOR type (keyword)
#[test]
fn test_create_tag_parser_vector_keyword() {
    let query = "CREATE TAG Document(id: STRING, embedding: VECTOR(128))";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with VECTOR type parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-VEC-TYPE-002: Parse CREATE TAG with VECTOR type (identifier)
#[test]
fn test_create_tag_parser_vector_identifier() {
    let query = "CREATE TAG Product(id: STRING, embedding: VECTOR(256))";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with VECTOR type (identifier format) parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-VEC-TYPE-003: Parse CREATE TAG with multiple VECTOR fields
#[test]
fn test_create_tag_parser_multiple_vector_fields() {
    let query =
        "CREATE TAG MultiVector(id: STRING, title_emb: VECTOR(128), content_emb: VECTOR(256))";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with multiple VECTOR fields parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-VEC-TYPE-004: Execute CREATE TAG with VECTOR type
#[test]
fn test_create_tag_execution_vector() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Document(vid: INT, id: STRING, embedding: VECTOR(128))")
        .assert_success()
        .assert_tag_exists("Document");
}

// ==================== Mixed Extended Types Tests ====================

/// TC-EXT-TYPE-001: Parse CREATE TAG with mixed extended types
#[test]
fn test_create_tag_parser_mixed_extended_types() {
    let query = "CREATE TAG Article(id: STRING, content: STRING, location: GEOGRAPHY, embedding: VECTOR(128))";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with mixed extended types parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

/// TC-EXT-TYPE-002: Execute CREATE TAG with mixed extended types
#[test]
fn test_create_tag_execution_mixed_extended_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Article(vid: INT, id: STRING, content: STRING, location: GEOGRAPHY, embedding: VECTOR(128))")
        .assert_success()
        .assert_tag_exists("Article");
}

/// TC-EXT-TYPE-003: Parse CREATE TAG with all standard and extended types
#[test]
fn test_create_tag_parser_all_types() {
    let query = r#"CREATE TAG AllTypes(
        name: STRING,
        age: INT,
        score: DOUBLE,
        active: BOOL,
        birth: DATE,
        created: TIMESTAMP,
        location: GEOGRAPHY,
        embedding: VECTOR(64)
    )"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with all types parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

// ==================== TTL Parser Tests ====================

#[test]
fn test_create_tag_parser_with_ttl() {
    let query = "CREATE TAG UserSession(sid: INT64, token: STRING, created_at: INT64, ttl_duration=31536000, ttl_col=created_at)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with TTL parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE TAG statement parsing should succeed");
    match stmt.ast.stmt() {
        linkrs_query::parser::ast::Stmt::Create(create) => match &create.target {
            linkrs_query::parser::ast::CreateTarget::Tag {
                ttl_duration,
                ttl_col,
                ..
            } => {
                assert_eq!(*ttl_duration, Some(31536000));
                assert_eq!(ttl_col.as_deref(), Some("created_at"));
            }
            other => panic!("expected tag target, got {:?}", other),
        },
        other => panic!("expected create statement, got {:?}", other),
    }
}

#[test]
fn test_create_tag_parser_with_ttl_quoted_col() {
    let query = r#"CREATE TAG UserSession(token: STRING, created_at: INT64, ttl_duration=100, ttl_col="created_at")"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "CREATE TAG with quoted ttl_col parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_tag_parser_with_half_ttl_rejected() {
    for query in [
        "CREATE TAG Bad1(x: INT, ttl_duration=100)",
        "CREATE TAG Bad2(x: INT, ttl_col=x)",
    ] {
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_err(),
            "one-sided TTL should be rejected: {}",
            query
        );
        let collected = format!("{:?}", parser.take_errors());
        assert!(
            collected.contains("both ttl_duration and ttl_col"),
            "TTL rejection should name both ttl_duration and ttl_col for {}: {}",
            query,
            collected
        );
    }
}

#[test]
fn test_create_tag_parser_with_non_positive_ttl_rejected() {
    let query = "CREATE TAG Bad(x: INT, ttl_duration=0, ttl_col=x)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_err(),
        "non-positive ttl_duration should be rejected: {:?}",
        result.ok()
    );
}

// ==================== TTL Execution Tests ====================

#[test]
fn test_create_tag_execution_with_ttl() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG UserSession(sid: INT64, token: STRING, created_at: INT64, ttl_duration=31536000, ttl_col=created_at)")
        .assert_success()
        .assert_tag_exists("UserSession");
}

#[test]
fn test_create_tag_execution_with_timestamp_ttl_col() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG UserSession(sid: INT64, token: STRING, created_at: TIMESTAMP, ttl_duration=31536000, ttl_col=created_at)")
        .assert_success()
        .assert_tag_exists("UserSession");
}

#[test]
fn test_create_tag_execution_with_ttl_type_mismatch_rejected() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Bad(name: STRING, ttl_duration=100, ttl_col=name)")
        .assert_error();
}

#[test]
fn test_create_tag_execution_with_ttl_unknown_col_rejected() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Bad(x: INT, ttl_duration=100, ttl_col=missing)")
        .assert_error();
}

#[test]
fn test_ttl_expired_vertex_is_filtered() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG UserSession(sid: INT64, token: STRING, created_at: INT64, ttl_duration=100, ttl_col=created_at)")
        .assert_success()
        .exec_dml("INSERT VERTEX UserSession(token, created_at) VALUES 1:('old', 0)")
        .assert_success()
        .exec_dml("INSERT VERTEX UserSession(token, created_at) VALUES 2:('fresh', 9999999999)")
        .assert_success()
        .assert_vertex_not_exists(1, "UserSession")
        .assert_vertex_exists(2, "UserSession");
}

#[test]
fn test_ttl_purge_removes_expired_vertices() {
    use linkrs_query::storage::{StorageReader, StorageSchemaOps, StorageWriter};

    let holder = common::TestStorage::new().expect("Failed to create test storage");
    let storage = holder.storage();
    {
        let mut guard = storage.write();
        let mut space = linkrs_core::types::SpaceInfo::new("ttl_space".to_string());
        StorageSchemaOps::create_space(&mut *guard, &mut space).expect("create space");
        let tag = linkrs_core::types::TagInfo::new("T".to_string())
            .with_properties(vec![
                linkrs_core::types::PropertyDef::new(
                    "id".to_string(),
                    linkrs_core::DataType::BigInt,
                ),
                linkrs_core::types::PropertyDef::new(
                    "ts".to_string(),
                    linkrs_core::DataType::BigInt,
                ),
            ])
            .with_ttl(Some(100), Some("ts".to_string()));
        StorageSchemaOps::create_tag(&mut *guard, "ttl_space", &tag).expect("create tag");
        let vid_old = linkrs_core::types::VertexId::try_from_int64(1).expect("vid");
        let vid_fresh = linkrs_core::types::VertexId::try_from_int64(2).expect("vid");
        let mut old_props = std::collections::HashMap::new();
        old_props.insert(
            std::sync::Arc::<str>::from("id"),
            linkrs_core::Value::BigInt(1),
        );
        old_props.insert(
            std::sync::Arc::<str>::from("ts"),
            linkrs_core::Value::BigInt(0),
        );
        let mut fresh_props = std::collections::HashMap::new();
        fresh_props.insert(
            std::sync::Arc::<str>::from("id"),
            linkrs_core::Value::BigInt(2),
        );
        fresh_props.insert(
            std::sync::Arc::<str>::from("ts"),
            linkrs_core::Value::BigInt(9999999999),
        );
        StorageWriter::insert_vertex(
            &mut *guard,
            "ttl_space",
            linkrs_core::Vertex::new(vid_old, linkrs_core::Tag::new("T".to_string(), old_props)),
        )
        .expect("insert old vertex");
        StorageWriter::insert_vertex(
            &mut *guard,
            "ttl_space",
            linkrs_core::Vertex::new(
                vid_fresh,
                linkrs_core::Tag::new("T".to_string(), fresh_props),
            ),
        )
        .expect("insert fresh vertex");
    }
    {
        let guard = storage.read();
        let visible = guard.scan_vertices_by_tag("ttl_space", "T").expect("scan");
        assert_eq!(visible.len(), 1, "expired vertex must be filtered");
    }
    {
        let guard = storage.read();
        let removed = guard
            .purge_expired_vertices("ttl_space", "T")
            .expect("purge");
        assert_eq!(removed, 1, "purge must remove exactly the expired vertex");
    }
}

// ==================== PURGE EXPIRED Statement Tests ====================

#[test]
fn test_purge_expired_vertex_physical_delete() {
    // The expired vertex is already filtered from reads; the purge must
    // physically delete it and report the removed-row count, and a second
    // purge finds nothing left.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ttl_space")
        .exec_ddl("CREATE TAG UserSession(sid: INT64, tok: STRING, created_at: INT64, ttl_duration=100, ttl_col=created_at)")
        .assert_success()
        .exec_dml("INSERT VERTEX UserSession(tok, created_at) VALUES 1:('old', 0)")
        .assert_success()
        .exec_dml("INSERT VERTEX UserSession(tok, created_at) VALUES 2:('fresh', 9999999999)")
        .assert_success()
        .assert_vertex_not_exists(1, "UserSession")
        .assert_vertex_exists(2, "UserSession")
        .exec_ddl("PURGE EXPIRED TAG UserSession IN ttl_space")
        .assert_success()
        .assert_result_columns(&["space_name", "schema_name", "schema_type", "removed_rows"])
        .assert_result_contains(vec![
            Value::string("ttl_space".to_string()),
            Value::string("UserSession".to_string()),
            Value::string("TAG".to_string()),
            Value::BigInt(1),
        ])
        .exec_ddl("PURGE EXPIRED TAG UserSession IN ttl_space")
        .assert_success()
        .assert_result_contains(vec![Value::BigInt(0)]);
}

#[test]
fn test_purge_expired_all_schemas_without_target() {
    // Omitting the target scans every TTL-configured schema and reports one
    // row per schema.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ttl_space")
        .exec_ddl("CREATE TAG UserSession(sid: INT64, created_at: INT64, ttl_duration=100, ttl_col=created_at)")
        .assert_success()
        .exec_dml("INSERT VERTEX UserSession(created_at) VALUES 1:(0)")
        .assert_success()
        .exec_ddl("PURGE EXPIRED IN ttl_space")
        .assert_success()
        .assert_result_columns(&["space_name", "schema_name", "schema_type", "removed_rows"])
        .assert_result_contains(vec![
            Value::string("UserSession".to_string()),
            Value::string("TAG".to_string()),
            Value::BigInt(1),
        ]);
}

#[test]
fn test_purge_expired_without_ttl_reports_zero() {
    // A schema that exists but has no TTL is scanned and reports zero removed
    // rows rather than erroring.
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ttl_space")
        .exec_ddl("CREATE TAG Keep(sid: INT64)")
        .assert_success()
        .exec_dml("INSERT VERTEX Keep(sid) VALUES 1:(1)")
        .assert_success()
        .exec_ddl("PURGE EXPIRED TAG Keep IN ttl_space")
        .assert_success()
        .assert_result_contains(vec![
            Value::string("Keep".to_string()),
            Value::string("TAG".to_string()),
            Value::BigInt(0),
        ]);
}

#[test]
fn test_purge_expired_unknown_space_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .exec_ddl("PURGE EXPIRED IN no_such_space")
        .assert_error();
}

#[test]
fn test_purge_expired_unknown_target_errors() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("ttl_space")
        .exec_ddl("PURGE EXPIRED TAG Missing IN ttl_space")
        .assert_error();
}

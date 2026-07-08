//! DDL Tag Basic Tests
//!
//! Test coverage:
//! - CREATE TAG - Create vertex tag
//! - DROP TAG - Delete vertex tag
//! - DESC TAG - Describe tag schema

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;
use graphdb_query::query::parser::Parser;
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
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person");
}

#[test]
fn test_create_tag_execution_with_if_not_exists() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person");
}

#[test]
fn test_create_tag_execution_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
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
        .exec_ddl("CREATE TAG Person(name: STRING)")
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
        .exec_ddl("CREATE TAG Person(name: STRING)")
        .assert_success()
        .exec_ddl("DROP TAG IF EXISTS Person")
        .assert_success()
        .assert_tag_not_exists("Person");
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
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .assert_success()
        .query("DESCRIBE TAG Person")
        .assert_success()
        .assert_result_count(2)
        .assert_result_contains(vec![
            Value::String("name".into()),
            Value::String("STRING".into()),
        ])
        .assert_result_contains(vec![
            Value::String("age".into()),
            Value::String("INT".into()),
        ]);
}

#[test]
fn test_desc_execution_tag_with_constraints() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING NOT NULL, age: INT DEFAULT 0)")
        .assert_success()
        .query("DESCRIBE TAG Person")
        .assert_success()
        .assert_result_count(2);
}

// ==================== Tag Lifecycle Tests ====================

#[test]
fn test_ddl_tag_lifecycle() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG TestTag(name: STRING, age: INT)")
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
        .exec_dml("DELETE VERTEX 1")
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
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name: STRING)")
        .assert_success()
        .assert_tag_exists("Person")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name: STRING)")
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
        .exec_ddl("CREATE TAG Location(name: STRING, coord: GEOGRAPHY)")
        .assert_success()
        .assert_tag_exists("Location");
}

/// TC-GEO-TYPE-004: Execute CREATE TAG with GEOGRAPHY and insert data
#[test]
fn test_create_tag_geography_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG City(name: STRING, center: GEOGRAPHY)")
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
        .exec_ddl("CREATE TAG Document(id: STRING, embedding: VECTOR(128))")
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
        .exec_ddl("CREATE TAG Article(id: STRING, content: STRING, location: GEOGRAPHY, embedding: VECTOR(128))")
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

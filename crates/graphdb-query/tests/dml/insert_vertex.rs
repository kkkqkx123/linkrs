//! DML Insert Vertex Tests
//!
//! Test coverage:
//! - INSERT VERTEX - Insert vertex data
//! - INSERT VERTEX IF NOT EXISTS
//! - INSERT vertex with all supported data types
//! - INSERT vertex with NULL values
//! - INSERT vertex without specifying properties

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::value::date_time::{DateTimeValue, DateValue};
use graphdb_query::core::Value;
use graphdb_query::query::parser::Parser;
use std::collections::HashMap;

// ==================== INSERT VERTEX Parser Tests ====================

#[test]
fn test_insert_parser_vertex() {
    let query = "INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT VERTEX parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_parser_multiple_vertices() {
    let query = "INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT multiple vertices parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_parser_invalid_syntax() {
    let query = "INSERT VERTEX Person(name, age) VALUES 1:'Alice', 30";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_err(), "Invalid syntax should trigger an error.");
}

// ==================== INSERT VERTEX Execution Tests ====================

#[test]
fn test_insert_execution_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
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

#[test]
fn test_insert_execution_multiple_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_count("Person", 2);
}

// ==================== INSERT IF NOT EXISTS Tests ====================

#[test]
fn test_insert_if_not_exists_parser() {
    let query = "INSERT VERTEX IF NOT EXISTS Person(name, age) VALUES 1:('Alice', 30)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT IF NOT EXISTS parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

#[test]
fn test_insert_if_not_exists_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .exec_dml("INSERT VERTEX IF NOT EXISTS Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        )
        .exec_dml("INSERT VERTEX IF NOT EXISTS Person(name, age) VALUES 1:('Bob', 25)")
        .assert_success()
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([
                ("name", Value::String("Alice".into())),
                ("age", Value::Int(30)),
            ]),
        );
}

// ==================== Multiple Tags Tests ====================

#[test]
fn test_insert_multiple_tags_parser() {
    let query = "INSERT VERTEX Person(name, age), Employee(department, salary) VALUES 1:('Alice', 30):('Engineering', 100000)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INSERT multiple tags parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INSERT statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "INSERT");
}

// ==================== Error Handling Tests ====================

#[test]
fn test_insert_duplicate_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Bob')")
        .assert_error();
}

#[test]
fn test_insert_vertex_with_all_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG TestTypes(
                str_field STRING,
                int_field INT,
                double_field DOUBLE,
                bool_field BOOL
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX TestTypes(str_field, int_field, double_field, bool_field) 
            VALUES 1:('test', 42, 2.71828, true)
        "#,
        )
        .assert_success()
        .assert_vertex_props(1, "TestTypes", {
            let mut map = HashMap::new();
            map.insert("str_field", Value::String("test".into()));
            map.insert("int_field", Value::Int(42));
            map.insert("double_field", Value::Double(2.71828));
            map.insert("bool_field", Value::Bool(true));
            map
        });
}

// ==================== Extended Data Types Tests ====================

#[test]
fn test_insert_vertex_with_date_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG DateTypes(
                date_field DATE
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX DateTypes(date_field) 
            VALUES 1:('2024-06-15')
        "#,
        )
        .assert_success()
        .assert_vertex_props(1, "DateTypes", {
            let mut map = HashMap::new();
            map.insert(
                "date_field",
                Value::Date(DateValue {
                    year: 2024,
                    month: 6,
                    day: 15,
                }),
            );
            map
        });
}

#[test]
fn test_insert_vertex_with_date_alternative_formats() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG AltDate(d DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX AltDate(d) VALUES 1:('2024/06/15')")
        .assert_success()
        .assert_vertex_props(
            1,
            "AltDate",
            HashMap::from([(
                "d",
                Value::Date(DateValue {
                    year: 2024,
                    month: 6,
                    day: 15,
                }),
            )]),
        );
}

#[test]
fn test_insert_vertex_with_datetime_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG DTTest(
                dt_field DATETIME
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX DTTest(dt_field) 
            VALUES 1:('2024-06-15 10:30:45')
        "#,
        )
        .assert_success()
        .assert_vertex_props(1, "DTTest", {
            let mut map = HashMap::new();
            map.insert(
                "dt_field",
                Value::DateTime(DateTimeValue {
                    year: 2024,
                    month: 6,
                    day: 15,
                    hour: 10,
                    minute: 30,
                    sec: 45,
                    microsec: 0,
                }),
            );
            map
        });
}

// ==================== Numeric Types Tests ====================

#[test]
fn test_insert_vertex_with_numeric_types() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG NumericTypes(
                int_field INT,
                float_field FLOAT
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX NumericTypes(int_field, float_field) 
            VALUES 1:(100, 3.14)
        "#,
        )
        .assert_success()
        .assert_vertex_props(1, "NumericTypes", {
            let mut map = HashMap::new();
            map.insert("int_field", Value::Int(100));
            map.insert("float_field", Value::Float(3.14_f32));
            map
        });
}

// ==================== NULL Values Tests ====================

#[test]
fn test_insert_vertex_with_null_values() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG NullableTypes(
                name STRING,
                age INT NULL,
                email STRING NULL
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX NullableTypes(name, age, email) 
            VALUES 1:('Alice', NULL, 'alice@example.com')
        "#,
        )
        .assert_success()
        .assert_vertex_exists(1, "NullableTypes");
}

#[test]
fn test_insert_vertex_with_partial_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl(
            r#"
            CREATE TAG PartialFields(
                name STRING,
                age INT NULL,
                email STRING NULL
            )
        "#,
        )
        .assert_success()
        .exec_dml(
            r#"
            INSERT VERTEX PartialFields(name) 
            VALUES 1:('Bob')
        "#,
        )
        .assert_success()
        .assert_vertex_exists(1, "PartialFields")
        .assert_vertex_props(
            1,
            "PartialFields",
            HashMap::from([("name", Value::String("Bob".into()))]),
        );
}

// ==================== FIXED_STRING Type Tests ====================

#[test]
fn test_insert_vertex_with_fixed_string() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG FixedStr(code STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX FixedStr(code) VALUES 1:('ABC123')")
        .assert_success()
        .assert_vertex_exists(1, "FixedStr");
}

// ==================== GEOGRAPHY Type Tests ====================

#[test]
fn test_insert_vertex_with_geography_type() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG GeoTypes(geo_field GEOGRAPHY)")
        .assert_success()
        .exec_dml("INSERT VERTEX GeoTypes(geo_field) VALUES 1:(NULL)")
        .assert_success()
        .assert_vertex_exists(1, "GeoTypes");
}

#[test]
fn test_insert_vertex_with_geography_and_other_fields() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Place(name STRING, location GEOGRAPHY)")
        .assert_success()
        .exec_dml("INSERT VERTEX Place(name, location) VALUES 1:('Beijing', NULL)")
        .assert_success()
        .assert_vertex_exists(1, "Place")
        .assert_vertex_props(
            1,
            "Place",
            HashMap::from([("name", Value::String("Beijing".into()))]),
        );
}

// ==================== Multi-Tag Insert Execution Tests ====================

#[test]
fn test_insert_multiple_tags_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE TAG Employee(department STRING, salary INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age), Employee(department, salary) VALUES 1:('Alice', 30):('Engineering', 100000)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(1, "Employee");
}

// ==================== Type Mismatch Error Tests ====================

#[test]
fn test_insert_type_mismatch_string_to_int() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG TestTypes(int_field INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX TestTypes(int_field) VALUES 1:('not_a_number')")
        .assert_error();
}

#[test]
fn test_insert_type_mismatch_bool_to_int() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG TestTypes(int_field INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX TestTypes(int_field) VALUES 1:(true)")
        .assert_error();
}

// ==================== Non-Existent Tag Error Tests ====================

#[test]
fn test_insert_vertex_nonexistent_tag() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_dml("INSERT VERTEX NonExistent(name) VALUES 1:('test')")
        .assert_error();
}

// ==================== Negative Vertex ID Tests ====================

#[test]
fn test_insert_vertex_negative_id() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES (-1):('Negative')")
        .assert_error();
}

// ==================== Empty String Property Tests ====================

#[test]
fn test_insert_vertex_empty_string() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_props(
            1,
            "Person",
            HashMap::from([("name", Value::String("".into()))]),
        );
}

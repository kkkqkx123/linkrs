//! DDL Schema Evolution Tests
//!
//! Test coverage:
//! - Schema evolution workflows
//! - Complex schema operations
//! - Multiple tags and edges

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::core::Value;
use std::collections::HashMap;

// ==================== Schema Evolution Tests ====================

#[test]
fn test_schema_evolution_complete_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG UserProfile(username STRING, created_at TIMESTAMP)")
        .assert_success()
        .exec_ddl("CREATE EDGE FOLLOWS(since DATE) FROM UserProfile TO UserProfile")
        .assert_success()
        .exec_dml("INSERT VERTEX UserProfile(username, created_at) VALUES 1:('alice', now())")
        .assert_success()
        .exec_dml("INSERT VERTEX UserProfile(username, created_at) VALUES 2:('bob', now())")
        .assert_success()
        .exec_dml("INSERT EDGE FOLLOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_vertex_count("UserProfile", 2)
        .assert_edge_count("FOLLOWS", 1)
        .exec_ddl("ALTER TAG UserProfile ADD (email STRING, bio STRING)")
        .assert_success()
        .exec_dml("UPDATE 1 SET email = 'alice@example.com', bio = 'Hello world'")
        .assert_success()
        .query("FETCH PROP ON UserProfile 1")
        .assert_result_count(1)
        .assert_vertex_props(1, "UserProfile", {
            let mut map = HashMap::new();
            map.insert("username", Value::String("alice".into()));
            map.insert("email", Value::String("alice@example.com".into()));
            map.insert("bio", Value::String("Hello world".into()));
            map
        });
}

#[test]
fn test_ddl_multiple_operations() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name: STRING, age: INT)")
        .assert_success()
        .exec_ddl("CREATE TAG Company(name: STRING, founded: INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE WORKS_AT(since: DATE) FROM Person TO Company")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since: DATE) FROM Person TO Person")
        .assert_success()
        .assert_tag_exists("Person")
        .assert_tag_exists("Company");
}

// ==================== Complex Schema Tests ====================

#[test]
fn test_complex_schema_with_multiple_tags_and_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("social_network")
        .exec_ddl(r#"
            CREATE TAG Person(
                name STRING,
                age INT,
                email STRING,
                created_at TIMESTAMP
            )
        "#)
        .assert_success()
        .exec_ddl(r#"
            CREATE TAG Company(
                name STRING,
                founded_year INT,
                industry STRING
            )
        "#)
        .assert_success()
        .exec_ddl(r#"
            CREATE EDGE KNOWS(
                since DATE,
                strength DOUBLE
            ) FROM Person TO Person
        "#)
        .assert_success()
        .exec_ddl(r#"
            CREATE EDGE WORKS_AT(
                since DATE,
                position STRING,
                salary DOUBLE
            ) FROM Person TO Company
        "#)
        .assert_success()
        .assert_tag_exists("Person")
        .assert_tag_exists("Company")
        .exec_dml("INSERT VERTEX Person(name, age, email) VALUES 1:('Alice', 30, 'alice@example.com')")
        .assert_success()
        .exec_dml("INSERT VERTEX Company(name, founded_year, industry) VALUES 101:('TechCorp', 2010, 'Technology')")
        .assert_success()
        .exec_dml("INSERT EDGE WORKS_AT(since, position, salary) VALUES 1 -> 101:('2020-01-01', 'Engineer', 100000.0)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(101, "Company")
        .assert_edge_exists(1, 101, "WORKS_AT");
}

// ==================== ALTER TAG CHANGE Tests ====================

#[test]
fn test_alter_tag_change_field() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(old_name STRING)")
        .assert_success()
        .exec_ddl("ALTER TAG Person CHANGE (old_name name: STRING)")
        .assert_success()
        .query("DESC TAG Person")
        .assert_result_count(1)
        .assert_result_contains(vec![
            Value::String("name".into()),
            Value::String("STRING".into()),
            Value::Bool(true),
            Value::String("".into()),
            Value::String("".into()),
        ]);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_create_tag_duplicate_field() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, name INT)")
        .assert_error();
}

#[test]
fn test_alter_tag_nonexistent_field() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_ddl("ALTER TAG Person DROP (nonexistent_field)")
        .assert_error();
}

#[test]
fn test_drop_tag_with_data() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .exec_ddl("DROP TAG Person")
        .assert_error();
}

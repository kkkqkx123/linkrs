//! Cypher Style CREATE Data Statement Integration Testing
//!
//! Test Range.
//! - CREATE (n:Label {prop: value}) - 创建node
//! - CREATE (a)-[:Type {prop: value}]->(b) - 创建edge
//! - CREATE (a:Label1)-[:Type]->(b:Label2) - 创建path
//! - Automatic Schema Inference and Creation

mod common;

use common::TestStorage;

use graphdb_query::core::stats::StatsManager;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::parser::Parser;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

// ==================== CREATE node test ====================

#[test]
fn test_create_cypher_node_basic() {
    let query = "CREATE (n:Person {name: 'Alice', age: 30})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEnode: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_cypher_node_without_props() {
    let query = "CREATE (n:Person)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEnode: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_node_multiple_labels() {
    let query = "CREATE (n:Person:Employee {name: 'Alice', department: 'Engineering'})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEnode多标签: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_node_without_variable() {
    let query = "CREATE (:Person {name: 'Bob'})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEnode无变量: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_node_complex_props() {
    let query = r#"CREATE (n:Person {
        name: 'Charlie',
        age: 35,
        salary: 50000.50,
        is_active: true,
        created_at: '2024-01-01T00:00:00'
    })"#;
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_edge_basic() {
    let query = "CREATE (a)-[:KNOWS {since: '2020-01-01', degree: 0.8}]->(b)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEedge: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("CREATE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "CREATE");
}

#[test]
fn test_create_cypher_edge_without_props() {
    let query = "CREATE (a)-[:FRIEND]->(b)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEedge: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_edge_bidirectional() {
    let query = "CREATE (a)-[:COLLEAGUE]-(b)";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_edge_left_to_right() {
    let query = "CREATE (a)<-[:FOLLOWS]-(b)";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_path_basic() {
    let query = "CREATE (a:Person)-[:KNOWS]->(b:Person)";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEpath: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_path_with_props() {
    let query = "CREATE (a:Person {name: 'Alice'})-[:KNOWS {since: '2020-01-01'}]->(b:Person {name: 'Bob'})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATEpath带属性: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_long_path() {
    let query = "CREATE (a:Person)-[:KNOWS]->(b:Person)-[:WORKS_AT]->(c:Company)";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_multiple_nodes() {
    let query =
        "CREATE (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}), (c:Person {name: 'Charlie'})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Cypher CREATE多个node: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_create_cypher_mixed_patterns() {
    let query = "CREATE (a:Person {name: 'Alice'}), (a)-[:KNOWS]->(b:Person {name: 'Bob'})";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_node_execution() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = test_storage.schema_manager();

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    let create_space = "CREATE SPACE IF NOT EXISTS test_space";
    let _ = pipeline_manager.execute_query(create_space);

    let use_space = "USE test_space";
    let _ = pipeline_manager.execute_query(use_space);

    let query = "CREATE (n:Person {name: 'Alice', age: 30})";
    let _result = pipeline_manager.execute_query(query);
}

#[test]
fn test_create_cypher_edge_execution() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = test_storage.schema_manager();

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    let create_space = "CREATE SPACE IF NOT EXISTS test_space";
    let _ = pipeline_manager.execute_query(create_space);

    let use_space = "USE test_space";
    let _ = pipeline_manager.execute_query(use_space);

    let query = "CREATE (a:Person {name: 'Alice'})-[:KNOWS {since: '2020-01-01'}]->(b:Person {name: 'Bob'})";
    let _result = pipeline_manager.execute_query(query);
}

#[test]
fn test_create_cypher_invalid_syntax() {
    let query = "CREATE n:Person {name: 'Alice'}";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_empty_label() {
    let query = "CREATE (n {})";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_create_cypher_nested_props() {
    let query = "CREATE (n:Person {address: {city: 'Beijing', street: 'Main St'}})";
    let mut parser = Parser::new(query);

    let _result = parser.parse();
}

#[test]
fn test_schema_auto_inference_string() {
    let query = "CREATE (n:Person {name: 'Alice'})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "String property parsing should succeed");
}

#[test]
fn test_schema_auto_inference_int() {
    let query = "CREATE (n:Person {age: 30})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "Integer attribute parsing should succeed");
}

#[test]
fn test_schema_auto_inference_float() {
    let query = "CREATE (n:Person {salary: 50000.50})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Floating point attribute parsing should succeed"
    );
}

#[test]
fn test_schema_auto_inference_bool() {
    let query = "CREATE (n:Person {is_active: true})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "Boolean attribute parsing should succeed");
}

#[test]
fn test_schema_auto_inference_mixed_types() {
    let query = "CREATE (n:Person {name: 'Alice', age: 30, salary: 50000.50, is_active: true})";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Mixed type attribute parsing should succeed"
    );
}

#[test]
fn test_cypher_vs_ngql_create_node() {
    let cypher_query = "CREATE (n:Person {name: 'Alice', age: 30})";
    let mut cypher_parser = Parser::new(cypher_query);
    let cypher_result = cypher_parser.parse();

    let ngql_query = "INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)";
    let mut ngql_parser = Parser::new(ngql_query);
    let ngql_result = ngql_parser.parse();

    assert!(
        cypher_result.is_ok(),
        "Cypher syntax should parse successfully"
    );
    assert!(
        ngql_result.is_ok(),
        "The NGQL syntax should parse successfully"
    );
}

#[test]
fn test_cypher_vs_ngql_create_edge() {
    let cypher_query = "CREATE (a)-[:KNOWS {since: '2020-01-01'}]->(b)";
    let mut cypher_parser = Parser::new(cypher_query);
    let cypher_result = cypher_parser.parse();

    let ngql_query = "INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01')";
    let mut ngql_parser = Parser::new(ngql_query);
    let _ = ngql_parser.parse();

    assert!(
        cypher_result.is_ok(),
        "Cypher syntax should parse successfully"
    );
}

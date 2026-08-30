//! DML Batch Operations Tests
//!
//! Test coverage:
//! - Batch INSERT operations
//! - Batch UPDATE operations
//! - Batch DELETE operations
//! - Complex DML workflows
//! - DML shape template AST cache

use super::common;

use common::test_scenario::TestScenario;
use common::TestStorage;
use graphdb_core::stats::StatsManager;
use graphdb_core::types::VertexId;
use graphdb_query::optimizer::OptimizerEngine;
use graphdb_query::pipeline::QueryPipelineManager;
use graphdb_query::storage::StorageReader;
use std::sync::Arc;

// ==================== Batch INSERT Tests ====================

#[test]
fn test_batch_insert_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml(
            r#"
            INSERT VERTEX Person(name, age) VALUES 
                1:('Alice', 30),
                2:('Bob', 25),
                3:('Charlie', 35),
                4:('Diana', 28),
                5:('Eve', 32)
        "#,
        )
        .assert_success()
        .assert_vertex_count("Person", 5);
}

#[test]
fn test_batch_insert_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D')")
        .exec_dml(
            r#"
            INSERT EDGE KNOWS(since) VALUES 
                1 -> 2:('2024-01-01'),
                1 -> 3:('2024-01-02'),
                1 -> 4:('2024-01-03'),
                2 -> 3:('2024-01-04'),
                2 -> 4:('2024-01-05')
        "#,
        )
        .assert_success()
        .assert_edge_count("KNOWS", 5);
}

// ==================== Batch DELETE Tests ====================

#[test]
fn test_batch_delete_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D'), 5:('E')")
        .assert_success()
        .assert_vertex_count("Person", 5)
        .exec_dml("DELETE VERTEX 1, 2, 3")
        .assert_success()
        .assert_vertex_count("Person", 2);
}

#[test]
fn test_batch_delete_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('A'), 2:('B'), 3:('C')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-01-02'), 2 -> 3:('2024-01-03')")
        .assert_success()
        .assert_edge_count("KNOWS", 3)
        .exec_dml("DELETE EDGE 1 -> 2, 1 -> 3 OF KNOWS")
        .assert_success()
        .assert_edge_count("KNOWS", 1);
}

// ==================== Complex DML Workflow Tests ====================

#[test]
fn test_dml_workflow_complete() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .exec_dml("UPDATE 1 SET age = 31")
        .assert_success()
        .exec_dml("DELETE EDGE 1 -> 2 OF KNOWS")
        .assert_success()
        .exec_dml("DELETE VERTEX 1, 2")
        .assert_success();
}

#[test]
fn test_dml_error_handling() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let invalid_queries = vec![
        "INSERT VERTEX Person(name) VALUES 1:",
        "INSERT EDGE KNOWS(since) VALUES 1 -> :('2024-01-01')",
        "UPDATE SET name = 'test'",
        "DELETE VERTEX",
        "DELETE EDGE",
    ];

    for query in invalid_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_err(),
            "Invalid query should return error: {}",
            query
        );
    }
}

// ==================== Complete CRUD Flow Tests ====================

#[test]
fn test_complete_crud_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Product(name STRING, price DOUBLE, stock INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Product(name, price, stock) VALUES 101:('Laptop', 999.99, 10)")
        .assert_success()
        .assert_vertex_exists(101, "Product")
        .assert_vertex_props(101, "Product", {
            let mut map = std::collections::HashMap::new();
            map.insert("stock", graphdb_core::Value::Int(10));
            map
        })
        .query("FETCH PROP ON Product 101")
        .assert_result_count(1)
        .assert_vertex_or_edge_has_property("name", graphdb_core::Value::string("Laptop"))
        .exec_dml("UPDATE 101 SET stock = 9")
        .assert_success()
        .assert_vertex_props(101, "Product", {
            let mut map = std::collections::HashMap::new();
            map.insert("stock", graphdb_core::Value::Int(9));
            map
        })
        .exec_dml("DELETE VERTEX 101")
        .assert_success()
        .assert_vertex_not_exists(101, "Product");
}

#[test]
fn test_social_network_data_flow() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("social_network")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35), 4:('David', 28)")
        .assert_success()
        .assert_vertex_count("Person", 4)
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2:('2020-01-01', 0.9), 1 -> 3:('2021-01-01', 0.8), 2 -> 3:('2020-06-01', 0.7), 3 -> 4:('2022-01-01', 0.9)")
        .assert_success()
        .assert_edge_count("KNOWS", 4)
        .query("GO FROM 1 OVER KNOWS YIELD $$.Person.name AS friend_name")
        .assert_success()
        .assert_result_count(2)
        .exec_dml("UPDATE 1 -> 2 OF KNOWS SET strength = 1.0")
        .assert_success()
        .exec_dml("DELETE EDGE KNOWS 2 -> 3")
        .assert_success()
        .assert_edge_not_exists(2, 3, "KNOWS")
        .assert_edge_count("KNOWS", 3);
}

// ==================== Performance Tests ====================

#[test]
fn test_large_batch_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml(
            r#"
            INSERT VERTEX Person(name) VALUES 
                1:('P1'), 2:('P2'), 3:('P3'), 4:('P4'), 5:('P5'),
                6:('P6'), 7:('P7'), 8:('P8'), 9:('P9'), 10:('P10')
        "#,
        )
        .assert_success()
        .assert_vertex_count("Person", 10);
}

// ==================== DML Shape Template AST Cache ====================

/// the shape-normalized DML template AST must be parsed at most once
/// per distinct template, and the cached AST must not misalign statement
/// parameters with column values.
#[test]
fn test_dml_shape_template_ast_cache() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());
    let schema_manager = test_storage.schema_manager();

    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    )
    .with_schema_manager(schema_manager);

    pipeline
        .execute_query_with_space("CREATE SPACE IF NOT EXISTS cache_space", None)
        .expect("create space");
    let space = storage
        .read()
        .get_space("cache_space")
        .expect("space lookup")
        .expect("space exists");
    pipeline
        .execute_query_with_space(
            "CREATE TAG person(name STRING, age INT)",
            Some(space.clone()),
        )
        .expect("create tag");

    // Statement 1: cold — the template is parsed and cached.
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX person(name, age) VALUES \"p1\": (\"A\", 1)",
            Some(space.clone()),
        )
        .expect("insert p1");
    assert_eq!(
        pipeline.dml_template_ast_parse_count(),
        1,
        "first statement must parse the template exactly once"
    );

    // Statement 2: same shape — the cached template AST is reused.
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX person(name, age) VALUES \"p2\": (\"B\", 2)",
            Some(space.clone()),
        )
        .expect("insert p2");
    assert_eq!(
        pipeline.dml_template_ast_parse_count(),
        1,
        "same-shape statement must reuse the cached template AST"
    );

    // Statement 3: heterogeneous shape (different column subset) — a new
    // template is parsed, replacing the cached slot.
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX person(name) VALUES \"p3\": (\"C\")",
            Some(space.clone()),
        )
        .expect("insert p3");
    assert_eq!(
        pipeline.dml_template_ast_parse_count(),
        2,
        "heterogeneous shape must parse a new template"
    );

    // The two same-shape statements must have landed their own parameter
    // values (the cached AST must not shift values across statements).
    let read = |vid: &str| {
        storage
            .read()
            .get_vertex("cache_space", &VertexId::from_string(vid))
            .expect("vertex read")
            .expect("vertex exists")
    };
    let p1 = read("p1");
    assert_eq!(
        p1.properties.get("name"),
        Some(&graphdb_core::Value::string("A"))
    );
    assert_eq!(
        p1.properties.get("age"),
        Some(&graphdb_core::Value::BigInt(1))
    );
    let p2 = read("p2");
    assert_eq!(
        p2.properties.get("name"),
        Some(&graphdb_core::Value::string("B"))
    );
    assert_eq!(
        p2.properties.get("age"),
        Some(&graphdb_core::Value::BigInt(2))
    );
    let p3 = read("p3");
    assert_eq!(
        p3.properties.get("name"),
        Some(&graphdb_core::Value::string("C"))
    );
}

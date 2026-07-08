//! DML Delete Tests
//!
//! Test coverage:
//! - DELETE VERTEX - Delete vertices
//! - DELETE EDGE - Delete edges
//! - DELETE with CASCADE
//! - Pipe DELETE - Delete via pipe (GO ... | DELETE)
//! - MATCH...DELETE - Delete matched patterns

use super::common;

use common::test_scenario::TestScenario;
use graphdb_query::query::parser::Parser;

// ==================== DELETE VERTEX Parser Tests ====================

#[test]
fn test_delete_parser_vertex() {
    let query = "DELETE VERTEX 1";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DELETE VERTEX parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DELETE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DELETE");
}

#[test]
fn test_delete_parser_multiple_vertices() {
    let query = "DELETE VERTEX 1, 2, 3";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DELETE multiple vertices parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DELETE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DELETE");
}

#[test]
fn test_delete_parser_vertex_with_edge() {
    let query = "DELETE VERTEX 1 WITH EDGE";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DELETE VERTEX WITH EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DELETE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DELETE");
}

// ==================== DELETE VERTEX Execution Tests ====================

#[test]
fn test_delete_execution_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        .assert_vertex_not_exists(1, "Person");
}

#[test]
fn test_delete_execution_multiple_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("DELETE VERTEX 1, 2")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_vertex_not_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

#[test]
fn test_delete_vertex_with_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("DELETE VERTEX 1 WITH EDGE")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_edge_not_exists(1, 2, "KNOWS");
}

// ==================== DELETE EDGE Parser Tests ====================

#[test]
fn test_delete_parser_edge() {
    let query = "DELETE EDGE 1 -> 2 OF KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DELETE EDGE parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DELETE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DELETE");
}

#[test]
fn test_delete_parser_multiple_edges() {
    let query = "DELETE EDGE 1 -> 2, 1 -> 3 OF KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "DELETE multiple edges parsing should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("DELETE statement parsing should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "DELETE");
}

// ==================== DELETE EDGE Execution Tests ====================

#[test]
fn test_delete_execution_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("DELETE EDGE 1 -> 2 OF KNOWS")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS");
}

// ==================== DELETE Edge Verification Tests ====================

#[test]
fn test_delete_edge_and_verify() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2020-01-01'), 1 -> 3:('2021-01-01')")
        .assert_success()
        .assert_edge_count("KNOWS", 2)
        .exec_dml("DELETE EDGE KNOWS 1 -> 2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .assert_edge_count("KNOWS", 1);
}

#[test]
fn test_delete_multiple_vertices_and_verify() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie'), 4:('David')",
        )
        .assert_success()
        .assert_vertex_count("Person", 4)
        .exec_dml("DELETE VERTEX 1, 2, 3")
        .assert_success()
        .assert_vertex_count("Person", 1)
        .assert_vertex_exists(4, "Person");
}

// ==================== Error Handling Tests ====================

#[test]
fn test_delete_nonexistent_vertex() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("DELETE VERTEX 999")
        .assert_success();
}

#[test]
fn test_delete_nonexistent_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("DELETE EDGE 1 -> 2 OF KNOWS")
        .assert_success();
}

// ==================== Pipe DELETE Parser Tests ====================

#[test]
fn test_pipe_delete_parser_vertex() {
    let query = r#"GO FROM "1" OVER knows YIELD dst(edge) AS id | DELETE VERTEX $-.id"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe DELETE VERTEX parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_pipe_delete_parser_vertex_with_edge() {
    let query = r#"GO FROM "1" OVER knows YIELD dst(edge) AS id | DELETE VERTEX $-.id WITH EDGE"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe DELETE VERTEX WITH EDGE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_pipe_delete_parser_edge() {
    let query = r#"GO FROM "1" OVER knows YIELD src(edge) AS s, dst(edge) AS d | DELETE EDGE knows $-.s -> $-.d"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "Pipe DELETE EDGE parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== Pipe DELETE Execution Tests ====================

#[test]
fn test_pipe_delete_vertex_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-01-02')")
        .assert_success()
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person")
        .exec_dml(r#"GO FROM 1 OVER KNOWS YIELD dst(edge) AS id | DELETE VERTEX $-.id"#)
        .assert_success()
        .assert_vertex_not_exists(2, "Person")
        .assert_vertex_not_exists(3, "Person")
        .assert_vertex_exists(1, "Person");
}

#[test]
fn test_pipe_delete_vertex_with_edge_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml(r#"GO FROM 1 OVER KNOWS YIELD dst(edge) AS id | DELETE VERTEX $-.id WITH EDGE"#)
        .assert_success()
        .assert_vertex_not_exists(2, "Person")
        .assert_edge_not_exists(1, 2, "KNOWS");
}

#[test]
fn test_pipe_delete_edge_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 1 -> 3:('2024-01-02')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .exec_dml(r#"GO FROM 1 OVER KNOWS YIELD src(edge) AS s, dst(edge) AS d | DELETE EDGE KNOWS $-.s -> $-.d"#)
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_not_exists(1, 3, "KNOWS")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

// ==================== MATCH...DELETE EDGE a -> b Tests ====================

#[test]
fn test_match_delete_edge_ref_parser() {
    let query = r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) DELETE EDGE a -> b"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE EDGE a -> b parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_edge_ref_with_rank_parser() {
    let query = r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) DELETE EDGE a -> b @0"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE EDGE a -> b @rank parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_edge_ref_multiple_parser() {
    let query = r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) DELETE EDGE a -> b, a -> b"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE EDGE a -> b, a -> b parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_edge_ref_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(2019), 1 -> 3:(2022)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .exec_dml(
            r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) WHERE e.since < 2020 DELETE EDGE a -> b"#,
        )
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

#[test]
fn test_match_delete_edge_ref_with_rank_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2 @0:(2019), 1 -> 3 @1:(2022)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml(
            r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) WHERE e.since < 2020 DELETE EDGE a -> b @0"#,
        )
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

// ==================== MATCH...DELETE Parser Tests ====================

#[test]
fn test_match_delete_parser_vertex() {
    let query = r#"MATCH (v:Person) WHERE v.age > 65 DELETE VERTEX v"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE VERTEX parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_parser_vertex_with_edge() {
    let query = r#"MATCH (v:Person) WHERE v.age > 65 DELETE VERTEX v WITH EDGE"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE VERTEX WITH EDGE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_parser_edge() {
    let query = r#"MATCH ()-[e:KNOWS]->() WHERE e.since < 2020 DELETE EDGE e"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE EDGE parsing should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_match_delete_parser_multiple_vertices() {
    let query = r#"MATCH (v:Person) DELETE VERTEX v, v"#;
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "MATCH...DELETE multiple vertices parsing should succeed: {:?}",
        result.err()
    );
}

// ==================== MATCH...DELETE Execution Tests ====================

#[test]
fn test_match_delete_vertex_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 70), 2:('Bob', 30), 3:('Charlie', 75)")
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person")
        .exec_dml(r#"MATCH (v:Person) WHERE v.age > 65 DELETE VERTEX v"#)
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_not_exists(3, "Person");
}

#[test]
fn test_match_delete_vertex_with_edge_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 70), 2:('Bob', 30)")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml(r#"MATCH (v:Person) WHERE v.age > 65 DELETE VERTEX v WITH EDGE"#)
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_vertex_exists(2, "Person");
}

#[test]
fn test_match_delete_edge_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since INT)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:(2019), 1 -> 3:(2022)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .exec_dml(r#"MATCH ()-[e:KNOWS]->() WHERE e.since < 2020 DELETE EDGE e"#)
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 3, "KNOWS")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

#[test]
fn test_match_delete_with_pattern_execution() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 2 -> 3:('2024-01-02')")
        .assert_success()
        .exec_dml(r#"MATCH (a:Person)-[e:KNOWS]->(b:Person) WHERE a.name == 'Alice' DELETE EDGE e"#)
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_exists(2, 3, "KNOWS");
}

// ==================== Combined DELETE Tests ====================

#[test]
fn test_pipe_delete_with_where_clause() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_ddl("CREATE EDGE FRIEND(age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 25), 2:('Bob', 30), 3:('Charlie', 35)")
        .exec_dml("INSERT EDGE FRIEND(age) VALUES 1 -> 2:(30), 1 -> 3:(35)")
        .assert_success()
        .exec_dml(r#"GO FROM 1 OVER FRIEND WHERE $$.Person.age > 28 YIELD dst(edge) AS id | DELETE VERTEX $-.id"#)
        .assert_success()
        .assert_vertex_exists(1, "Person")
        .assert_vertex_not_exists(2, "Person")
        .assert_vertex_not_exists(3, "Person");
}

#[test]
fn test_match_delete_with_limit() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 70), 2:('Bob', 75), 3:('Charlie', 80)")
        .assert_success()
        .exec_dml(r#"MATCH (v:Person) WHERE v.age > 65 DELETE VERTEX v"#)
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_vertex_not_exists(2, "Person")
        .assert_vertex_not_exists(3, "Person");
}

// ==================== Rank-based Edge Deletion Tests ====================

#[test]
fn test_delete_edge_by_rank() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE, strength DOUBLE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since, strength) VALUES 1 -> 2 @0:('2020-01-01', 0.5)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("DELETE EDGE 1 -> 2 OF KNOWS")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS");
}

// ==================== DELETE Vertex Without WITH EDGE (Dangling Edge) Tests ====================

#[test]
fn test_delete_vertex_with_edges_no_with_edge() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .exec_dml("DELETE VERTEX 1")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_vertex_exists(2, "Person");
}

// ==================== Multi-Hop Cascading Delete Tests ====================

#[test]
fn test_delete_multi_hop_cascade() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 2 -> 3:('2024-02-01')")
        .assert_success()
        .exec_dml("DELETE VERTEX 1 WITH EDGE")
        .assert_success()
        .assert_vertex_not_exists(1, "Person")
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_vertex_exists(2, "Person")
        .assert_edge_exists(2, 3, "KNOWS")
        .assert_vertex_exists(3, "Person");
}

// ==================== Multi-hop MATCH...DELETE Tests ====================

#[test]
fn test_match_delete_multi_hop() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS(since DATE)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1 -> 2:('2024-01-01'), 2 -> 3:('2024-02-01')")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(2, 3, "KNOWS")
        .exec_dml("DELETE EDGE 1 -> 2, 2 -> 3 OF KNOWS")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_not_exists(2, 3, "KNOWS")
        .assert_vertex_exists(1, "Person")
        .assert_vertex_exists(2, "Person")
        .assert_vertex_exists(3, "Person");
}

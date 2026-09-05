//! Managing and assisting statement integration testing
//!
//! Test Range.
//! - USE - Using the graph space
//! - SHOW - Show information (SPACES, TAGS, EDGES, HOSTS, PARTS, SESSIONS, QUERIES, CONFIGS)
//! - EXPLAIN - query plan (supports FORMAT = TABLE/DOT)
//! - PROFILE - Performance Analysis (FORMAT = TABLE/DOT supported)
//! - GROUP BY - grouping statement
//! - KILL QUERY - terminates the query
//! - UPDATE CONFIGS - Update Configuration
//! - RETURN - return result
//! - WITH - Intermediate Results Handling
//! - UNWIND - Expand List
//! - PIPE - Pipeline Operation

mod common;

use common::TestStorage;

use graphdb_core::stats::StatsManager;
use graphdb_query::optimizer::OptimizerEngine;
use graphdb_query::parser::Parser;
use graphdb_query::pipeline::QueryPipelineManager;
use std::sync::Arc;

// ==================== USE Statement Tests ====================

#[test]
fn test_use_parser_basic() {
    let query = "USE test_space";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE basic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE statement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_parser_complex_name() {
    let query = "USE my_graph_space_123";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE complex name: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE statement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_parser_with_dots() {
    let query = "USE db.graph.space";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE dotted name: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE statement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_execution_basic() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "USE test_space";
    let result = pipeline_manager.execute_query(query);

    match result {
        Ok(_) => panic!("USE of a missing space should not succeed"),
        Err(e) => assert!(
            format!("{e:?}").contains("Space not found"),
            "USE missing space should report Space not found, got: {e:?}"
        ),
    };
}

#[test]
fn test_use_execution_nonexistent() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "USE nonexistent_space_xyz";
    let result = pipeline_manager.execute_query(query);

    match result {
        Ok(_) => panic!("USE of a missing space should not succeed"),
        Err(e) => assert!(
            format!("{e:?}").contains("Space not found"),
            "USE missing space should report Space not found, got: {e:?}"
        ),
    };
}

// ==================== SHOW statement ====================

#[test]
fn test_show_parser_spaces() {
    let query = "SHOW SPACES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW SPACES: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_parser_tags() {
    let query = "SHOW TAGS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW TAGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_parser_edges() {
    let query = "SHOW EDGES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW EDGES: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_parser_hosts() {
    let query = "SHOW HOSTS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW HOSTS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_parser_parts() {
    let query = "SHOW PARTS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW PARTS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_execution_spaces() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW SPACES";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_show_execution_tags() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("show_tags_space")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .query("SHOW TAGS")
        .assert_success();
}

#[test]
fn test_show_execution_edges() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("show_edges_space")
        .exec_ddl("CREATE EDGE knows()")
        .assert_success()
        .query("SHOW EDGES")
        .assert_success();
}

// ==================== EXPLAIN statement ====================

#[test]
fn test_explain_parser_match() {
    let query = "EXPLAIN MATCH (n:Person) RETURN n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN MATCH: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_parser_go() {
    let query = "EXPLAIN GO FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN GO: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_parser_lookup() {
    let query = "EXPLAIN LOOKUP ON Person WHERE Person.name == 'Alice'";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN LOOKUP: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_execution_match() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN MATCH (n:Person) RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_explain_execution_go() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN GO FROM 1 OVER KNOWS";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_explain_analyze_parser() {
    let query = "EXPLAIN ANALYZE MATCH (n:Person) RETURN n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN ANALYZE: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAIN ANALYZEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
    assert!(
        stmt.ast
            .stmt
            .as_explain()
            .map(|e| e.analyze)
            .unwrap_or(false),
        "EXPLAIN ANALYZE should set the analyze flag"
    );
}

#[test]
fn test_explain_analyze_execution() {
    use common::test_scenario::TestScenario;

    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("explain_analyze_space")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\")")
        .assert_success()
        .query("EXPLAIN ANALYZE MATCH (n:person) RETURN n")
        .assert_success();

    let plan_text = scenario.get_plan_string().unwrap_or_default();
    assert!(
        plan_text.contains("rows:") && plan_text.contains("us"),
        "EXPLAIN ANALYZE output should contain per-operator rows/time, got: {}",
        plan_text
    );
}

// ==================== RETURN statement ====================

#[test]
fn test_return_parser_basic() {
    let query = "RETURN n.name, n.age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURNbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURNstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_alias() {
    let query = "RETURN n.name AS name, n.age AS age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURNwith alias: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURNstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_expression() {
    let query = "RETURN n.age * 2 AS double_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURNwith expression: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURNstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_aggregate() {
    let query = "RETURN count(*) AS total, avg(n.age) AS avg_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURNwith aggregate: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURNstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_distinct() {
    let query = "RETURN DISTINCT n.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURNDISTINCT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURNstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_execution_basic() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "RETURN 'Hello World'";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== WITH statement ====================

#[test]
fn test_with_parser_basic() {
    let query = "WITH n.name AS name, n.age AS age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITHbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITHstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_parser_with_aggregate() {
    let query = "WITH count(*) AS total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITHwith aggregation: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITHstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_parser_with_expression() {
    let query = "WITH n.age * 2 AS double_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITHwith expression: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITHstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_execution_basic() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "WITH 1 AS x RETURN x";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== UNWIND statement ====================

#[test]
fn test_unwind_parser_basic() {
    let query = "UNWIND [1, 2, 3] AS n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWINDbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWINDstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_parser_with_string_list() {
    let query = "UNWIND ['a', 'b', 'c'] AS s";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWINDstring list: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWINDstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_parser_with_expression() {
    let query = "UNWIND range(1, 10) AS n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWINDwith expression: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWINDstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_execution_basic() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "UNWIND [1, 2, 3] AS n RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== PIPE statement ====================

#[test]
fn test_pipe_parser_basic() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PIPEbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PIPEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_parser_multiple() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name | FETCH PROP ON Person $-.id";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PIPEmultiple ops: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PIPEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_parser_complex() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name AS name, target.age AS age WHERE age > 25 | RETURN name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "PIPE: should succeed: {:?}", result.err());

    let stmt = result.expect("PIPEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_execution_basic() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "GO FROM 1 OVER KNOWS | YIELD target.name";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== PROFILE Statement Tests ====================

#[test]
fn test_profile_parser_match() {
    let query = "PROFILE MATCH (n:Person) RETURN n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILE MATCH: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_parser_go() {
    let query = "PROFILE GO FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILE GO: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_parser_with_limit() {
    let query = "PROFILE MATCH (n:Person) RETURN n LIMIT 10";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILELIMIT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_execution_match() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("profile_match_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("PROFILE MATCH (n:Person) RETURN n")
        .assert_success();
}

#[test]
fn test_profile_execution_go() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("profile_go_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS()")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS() VALUES 1 -> 2")
        .assert_success()
        .query("PROFILE GO FROM 1 OVER KNOWS")
        .assert_success();
}

// ==================== GROUP BY Statement Tests ====================

#[test]
fn test_group_by_parser_basic() {
    let query = "GROUP BY category YIELD category, count(*) AS total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BYbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_parser_with_aggregation() {
    let query = "GROUP BY city YIELD city, avg(age) AS avg_age, max(age) AS max_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BYwith aggregate: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_execution_basic() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("group_by_space")
        .exec_ddl("CREATE TAG sales(category STRING, amount INT)")
        .exec_dml("INSERT VERTEX sales(category, amount) VALUES 1:('a', 10), 2:('b', 20)")
        .assert_success()
        .query("MATCH (s:sales) RETURN s.category, sum(s.amount) AS total GROUP BY s.category")
        .assert_success()
        .assert_result_count(2);
}

// ==================== KILL QUERY Statement Tests ====================

#[test]
fn test_kill_query_parser_basic() {
    let query = "KILL QUERY 123, 456";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "KILL QUERYbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("KILL QUERYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "KILL QUERY");
}

#[test]
fn test_kill_query_parser_multiple() {
    let query = "KILL QUERY 456, 789";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "KILL QUERYmultiple queries: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("KILL QUERYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "KILL QUERY");
}

#[test]
fn test_kill_query_execution() {
    // KILL QUERY is a server/session-layer operation: it parses in the
    // embedded pipeline but planning reports unsupported. Pin that contract.
    let mut parser = Parser::new("KILL QUERY 123, 456");
    let parsed = parser.parse();
    assert!(
        parsed.is_ok(),
        "KILL QUERY should parse: {:?}",
        parsed.err()
    );

    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let result = pipeline_manager.execute_query("KILL QUERY 123, 456");
    match result {
        Ok(_) => panic!("KILL QUERY should not execute in the embedded pipeline"),
        Err(e) => assert!(
            format!("{e:?}").contains("not supported"),
            "KILL QUERY should report unsupported, got: {e:?}"
        ),
    };
}

// ==================== UPDATE CONFIGS Statement Tests ====================

#[test]
fn test_update_configs_parser_basic() {
    let query = "UPDATE CONFIGS max_connections = 100";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGSbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_parser_with_module() {
    let query = "UPDATE CONFIGS storage cache_size = 1024";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_parser_multiple() {
    let query = "UPDATE CONFIGS max_connections = 100, timeout = 30";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGSmultiple: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_execution() {
    // UPDATE CONFIGS is a server-layer operation: it parses in the embedded
    // pipeline but planning reports unsupported. Pin that contract.
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let result = pipeline_manager.execute_query("UPDATE CONFIGS max_connections = 100");
    match result {
        Ok(_) => panic!("UPDATE CONFIGS should not execute in the embedded pipeline"),
        Err(e) => assert!(
            format!("{e:?}").contains("not supported"),
            "UPDATE CONFIGS should report unsupported, got: {e:?}"
        ),
    };
}

// ==================== SHOW SESSIONS/QUERIES/CONFIGS Tests ====================

#[test]
fn test_show_parser_sessions() {
    let query = "SHOW SESSIONS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW SESSIONS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW SESSIONS");
}

#[test]
fn test_show_parser_queries() {
    let query = "SHOW QUERIES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW QUERIES: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW QUERIES");
}

#[test]
fn test_show_parser_configs() {
    let query = "SHOW CONFIGS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW CONFIGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW CONFIGS");
}

#[test]
fn test_show_parser_configs_with_module() {
    let query = "SHOW CONFIGS storage";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW CONFIGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOWstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW CONFIGS");
}

#[test]
fn test_show_execution_sessions() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW SESSIONS";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_show_execution_queries() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW QUERIES";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_show_execution_configs() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW CONFIGS";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== EXPLAIN FORMAT Tests ====================

#[test]
fn test_explain_parser_format_table() {
    let query = "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN FORMAT TABLE: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_parser_format_dot() {
    let query = "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN FORMAT DOT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_execution_format_table() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

#[test]
fn test_explain_execution_format_dot() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS";
    let result = pipeline_manager.execute_query(query);

    assert!(
        result.is_ok(),
        "Execution should return Ok result: {:?}",
        result.err()
    );
}

// ==================== statement ====================

#[test]
fn test_management_show_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    // SHOW TAGS / SHOW EDGES require a selected space; they are covered
    // with space context in test_show_execution_tags/edges.
    let show_queries = [
        "SHOW SPACES",
        "SHOW HOSTS",
        "SHOW PARTS",
        "SHOW SESSIONS",
        "SHOW QUERIES",
        "SHOW CONFIGS",
    ];

    for query in &show_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_management_explain_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let explain_queries = [
        "EXPLAIN MATCH (n:Person) RETURN n",
        "EXPLAIN GO FROM 1 OVER KNOWS",
        "EXPLAIN LOOKUP ON Person WHERE Person.age > 25",
        "EXPLAIN FETCH PROP ON Person 1",
        "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n",
        "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS",
    ];

    for query in &explain_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_management_profile_operations() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("mgmt_profile_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("PROFILE MATCH (n:Person) RETURN n")
        .assert_success();
}

#[test]
fn test_management_group_by_operations() {
    // Standalone GROUP BY executes against grouped input; cover the
    // end-to-end aggregation shape through MATCH + GROUP BY with data.
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("mgmt_group_by_space")
        .exec_ddl("CREATE TAG sales(category STRING, amount INT)")
        .exec_dml(
            "INSERT VERTEX sales(category, amount) VALUES 1:('a', 10), 2:('a', 20), 3:('b', 5)",
        )
        .assert_success()
        .query("MATCH (s:sales) RETURN s.category, sum(s.amount) AS total GROUP BY s.category")
        .assert_success()
        .assert_result_count(2);

    // The legacy standalone GROUP BY syntax must still parse.
    for query in [
        "GROUP BY category YIELD category, count(*) AS total",
        "GROUP BY city YIELD city, avg(age) AS avg_age",
    ] {
        let mut parser = Parser::new(query);
        let result = parser.parse();
        assert!(
            result.is_ok(),
            "GROUP BY should parse '{query}': {:?}",
            result.err()
        );
    }
}

#[test]
fn test_management_kill_query_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    // KILL QUERY takes a (session, query) id pair and is executed by the
    // server layer; the embedded pipeline reports unsupported.
    let kill_queries = ["KILL QUERY 123, 456", "KILL QUERY 456, 789"];

    for query in &kill_queries {
        let result = pipeline_manager.execute_query(query);
        match result {
            Ok(_) => panic!("{query} should not execute in the embedded pipeline"),
            Err(e) => assert!(
                format!("{e:?}").contains("not supported"),
                "{query} should report unsupported, got: {e:?}"
            ),
        };
    }
}

#[test]
fn test_management_update_configs_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let update_configs_queries = [
        "UPDATE CONFIGS max_connections = 100",
        "UPDATE CONFIGS timeout = 30",
        "UPDATE CONFIGS storage cache_size = 1024",
    ];

    // UPDATE CONFIGS is a server-layer operation; the embedded pipeline
    // reports unsupported. Pin that contract per statement.
    for query in &update_configs_queries {
        let result = pipeline_manager.execute_query(query);
        match result {
            Ok(_) => panic!("{query} should not execute in the embedded pipeline"),
            Err(e) => assert!(
                format!("{e:?}").contains("not supported"),
                "{query} should report unsupported, got: {e:?}"
            ),
        };
    }
}

#[test]
fn test_auxiliary_return_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let return_queries = [
        "RETURN 'Hello'",
        "RETURN 1 + 2",
        "RETURN [1, 2, 3]",
        "RETURN {name: 'Alice', age: 30}",
    ];

    for query in return_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_auxiliary_unwind_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let unwind_queries = [
        "UNWIND [1, 2, 3] AS n RETURN n",
        "UNWIND ['a', 'b', 'c'] AS s RETURN s",
        "UNWIND [1, 2, 3] AS n RETURN n * 2",
    ];

    for query in unwind_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_auxiliary_pipe_operations() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("pipe_ops_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_ddl("CREATE EDGE KNOWS()")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .exec_dml("INSERT EDGE KNOWS() VALUES 1 -> 2")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS | YIELD target.name")
        .assert_success()
        .query("GO FROM 1 OVER KNOWS | YIELD target.name AS name | RETURN name")
        .assert_success()
        .assert_result_count(1);
}

#[test]
fn test_management_error_handling() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let invalid_queries = vec![
        "USE",              // Lack of space names
        "SHOW",             // Missing objects
        "EXPLAIN",          // Missing query
        "PROFILE",          // Missing query
        "RETURN",           // Missing expressions
        "UNWIND",           // Missing lists and variables
        "WITH",             // Missing expressions
        "GO FROM 1 OVER |", // PIPE syntax error
        "GROUP BY",         // Missing expressions
        "KILL QUERY",       // Missing query id
        "UPDATE CONFIGS",   // Missing configs
    ];

    for query in invalid_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_err(), ": {}", query);
    }
}

#[test]
fn test_management_combined_operations() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("mgmt_combined_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("SHOW TAGS")
        .assert_success()
        .query("SHOW SESSIONS")
        .assert_success()
        .query("SHOW QUERIES")
        .assert_success()
        .query("SHOW CONFIGS")
        .assert_success()
        .query("EXPLAIN MATCH (n:Person) RETURN n")
        .assert_success()
        .query("UNWIND [1, 2, 3] AS n RETURN n")
        .assert_success()
        .assert_result_count(3)
        .query("WITH 1 AS x RETURN x")
        .assert_success()
        .assert_result_count(1)
        .query("RETURN 'Complete'")
        .assert_success();
}

#[test]
fn test_auxiliary_with_operations() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let with_queries = [
        "WITH 1 AS x RETURN x",
        "WITH [1, 2, 3] AS nums RETURN nums",
        "WITH 'Hello' AS msg RETURN msg",
    ];

    for query in with_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_management_performance() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW SPACES";
    let iterations = 10;

    for _ in 0..iterations {
        let result = pipeline_manager.execute_query(query);
        assert!(
            result.is_ok(),
            "Execution should return Ok result: {:?}",
            result.err()
        );
    }
}

// ==================== EXPLAIN FORMAT statement ====================

#[test]
fn test_explain_format_table() {
    let query = "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN FORMAT TABLE: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_format_dot() {
    let query = "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "EXPLAIN FORMAT DOT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("EXPLAINstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_profile_statement() {
    let query = "PROFILE MATCH (n:Person) RETURN n LIMIT 10";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILE: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_format_dot() {
    let query = "PROFILE FORMAT = DOT GO FROM 1 OVER KNOWS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILE FORMAT DOT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILEstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

// ==================== GROUP BY statement ====================

#[test]
fn test_group_by_basic() {
    let query = "GROUP BY category YIELD category";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BYbasic: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_multiple_items() {
    let query = "GROUP BY category, type YIELD category, type";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

// ==================== Session Management Statement Test ====================

#[test]
fn test_show_sessions() {
    let query = "SHOW SESSIONS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW SESSIONS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW SESSIONSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW SESSIONS");
}

#[test]
fn test_show_queries() {
    let query = "SHOW QUERIES";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW QUERIES: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW QUERIESstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW QUERIES");
}

#[test]
fn test_kill_query() {
    let query = "KILL QUERY 123, 456";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "KILL QUERY: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("KILL QUERYstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "KILL QUERY");
}

// ==================== Configuration Management Statement Test ====================

#[test]
fn test_show_configs() {
    let query = "SHOW CONFIGS";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW CONFIGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW CONFIGS");
}

#[test]
fn test_show_configs_with_module() {
    let query = "SHOW CONFIGS storage";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW CONFIGS storage: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW CONFIGS");
}

#[test]
fn test_update_configs() {
    let query = "UPDATE CONFIGS max_connections = 100";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_with_module() {
    let query = "UPDATE CONFIGS storage cache_size = 1024";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS storage: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

// ==================== Comprehensive test of new features ====================

#[test]
fn test_new_management_features() {
    use crate::common::test_scenario::TestScenario;
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("new_mgmt_space")
        .exec_ddl("CREATE TAG Person(name STRING)")
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .query("EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n")
        .assert_success()
        .query("PROFILE MATCH (n:Person) RETURN n LIMIT 10")
        .assert_success()
        .query("SHOW SESSIONS")
        .assert_success()
        .query("SHOW QUERIES")
        .assert_success()
        .query("SHOW CONFIGS")
        .assert_success()
        .query("SHOW CONFIGS storage")
        .assert_success();
}

// ==================== Variable Assignment Statement Test ====================

#[test]
fn test_assignment_statement() {
    let query = "$result = GO FROM \"player100\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), ": should succeed: {:?}", result.err());

    let stmt = result.expect("statement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ASSIGNMENT");
}

// ==================== statement

#[test]
fn test_union_statement() {
    let query = "GO FROM \"player100\" OVER follow UNION GO FROM \"player101\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "UNION: should succeed: {:?}", result.err());

    let stmt = result.expect("UNIONstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SET OPERATION");
}

#[test]
fn test_intersect_statement() {
    let query = "GO FROM \"player100\" OVER follow INTERSECT GO FROM \"player101\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "INTERSECT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("INTERSECTstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SET OPERATION");
}

#[test]
fn test_minus_statement() {
    let query = "GO FROM \"player100\" OVER follow MINUS GO FROM \"player101\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "MINUS: should succeed: {:?}", result.err());

    let stmt = result.expect("MINUSstatement: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SET OPERATION");
}

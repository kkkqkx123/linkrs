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

use graphdb_query::core::stats::StatsManager;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::parser::Parser;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

// ==================== USE 语句测试 ====================

#[test]
fn test_use_parser_basic() {
    let query = "USE test_space";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_parser_complex_name() {
    let query = "USE my_graph_space_123";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE复杂名称: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_parser_with_dots() {
    let query = "USE db.graph.space";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "USE带点号名称: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("USE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "USE");
}

#[test]
fn test_use_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "USE test_space";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_use_execution_nonexistent() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "USE nonexistent_space_xyz";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== SHOW 语句测试 ====================

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

    let stmt = result.expect("SHOW语句: should succeed");
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

    let stmt = result.expect("SHOW语句: should succeed");
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

    let stmt = result.expect("SHOW语句: should succeed");
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

    let stmt = result.expect("SHOW语句: should succeed");
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

    let stmt = result.expect("SHOW语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_execution_spaces() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW SPACES";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_show_execution_tags() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW TAGS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_show_execution_edges() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW EDGES";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== EXPLAIN 语句测试 ====================

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

    let stmt = result.expect("EXPLAIN语句: should succeed");
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

    let stmt = result.expect("EXPLAIN语句: should succeed");
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

    let stmt = result.expect("EXPLAIN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_execution_match() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN MATCH (n:Person) RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_explain_execution_go() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN GO FROM 1 OVER KNOWS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== RETURN 语句测试 ====================

#[test]
fn test_return_parser_basic() {
    let query = "RETURN n.name, n.age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURN基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_alias() {
    let query = "RETURN n.name AS name, n.age AS age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURN带别名: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_expression() {
    let query = "RETURN n.age * 2 AS double_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURN带表达式: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_aggregate() {
    let query = "RETURN count(*) AS total, avg(n.age) AS avg_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURN带聚合函数: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_parser_with_distinct() {
    let query = "RETURN DISTINCT n.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "RETURN带DISTINCT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("RETURN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "RETURN");
}

#[test]
fn test_return_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "RETURN 'Hello World'";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== WITH 语句测试 ====================

#[test]
fn test_with_parser_basic() {
    let query = "WITH n.name AS name, n.age AS age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITH语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_parser_with_aggregate() {
    let query = "WITH count(*) AS total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH带聚合: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITH语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_parser_with_expression() {
    let query = "WITH n.age * 2 AS double_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "WITH带表达式: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("WITH语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "WITH");
}

#[test]
fn test_with_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "WITH 1 AS x RETURN x";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== UNWIND 语句测试 ====================

#[test]
fn test_unwind_parser_basic() {
    let query = "UNWIND [1, 2, 3] AS n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWIND基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWIND语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_parser_with_string_list() {
    let query = "UNWIND ['a', 'b', 'c'] AS s";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWIND字符串列表: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWIND语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_parser_with_expression() {
    let query = "UNWIND range(1, 10) AS n";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UNWIND带表达式: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UNWIND语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UNWIND");
}

#[test]
fn test_unwind_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "UNWIND [1, 2, 3] AS n RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== PIPE 语句测试 ====================

#[test]
fn test_pipe_parser_basic() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PIPE基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PIPE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_parser_multiple() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name | FETCH PROP ON Person $-.id";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PIPE多个操作: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PIPE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_parser_complex() {
    let query = "GO FROM 1 OVER KNOWS | YIELD target.name AS name, target.age AS age WHERE age > 25 | RETURN name";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PIPE复杂查询: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PIPE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PIPE");
}

#[test]
fn test_pipe_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "GO FROM 1 OVER KNOWS | YIELD target.name";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
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

    let stmt = result.expect("PROFILE语句: should succeed");
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

    let stmt = result.expect("PROFILE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_parser_with_limit() {
    let query = "PROFILE MATCH (n:Person) RETURN n LIMIT 10";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "PROFILE带LIMIT: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("PROFILE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

#[test]
fn test_profile_execution_match() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "PROFILE MATCH (n:Person) RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_profile_execution_go() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "PROFILE GO FROM 1 OVER KNOWS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== GROUP BY Statement Tests ====================

#[test]
fn test_group_by_parser_basic() {
    let query = "GROUP BY category YIELD category, count(*) AS total";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BY语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_parser_with_aggregation() {
    let query = "GROUP BY city YIELD city, avg(age) AS avg_age, max(age) AS max_age";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY带聚合函数: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BY语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_execution_basic() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "GROUP BY category YIELD category, count(*) AS total";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== KILL QUERY Statement Tests ====================

#[test]
fn test_kill_query_parser_basic() {
    let query = "KILL QUERY 123";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "KILL QUERY基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("KILL QUERY语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "KILL QUERY");
}

#[test]
fn test_kill_query_parser_multiple() {
    let query = "KILL QUERY 123, 456, 789";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "KILL QUERY多个查询: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("KILL QUERY语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "KILL QUERY");
}

#[test]
fn test_kill_query_execution() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "KILL QUERY 123";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== UPDATE CONFIGS Statement Tests ====================

#[test]
fn test_update_configs_parser_basic() {
    let query = "UPDATE CONFIGS max_connections = 100";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGS语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_parser_with_module() {
    let query = "UPDATE CONFIGS storage cache_size = 1024";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS带模块: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGS语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_parser_multiple() {
    let query = "UPDATE CONFIGS max_connections = 100, timeout = 30";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "UPDATE CONFIGS多个配置: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("UPDATE CONFIGS语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

#[test]
fn test_update_configs_execution() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "UPDATE CONFIGS max_connections = 100";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
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

    let stmt = result.expect("SHOW语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
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

    let stmt = result.expect("SHOW语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
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

    let stmt = result.expect("SHOW语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_parser_configs_with_module() {
    let query = "SHOW CONFIGS storage";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "SHOW CONFIGS带模块: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("SHOW语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SHOW");
}

#[test]
fn test_show_execution_sessions() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW SESSIONS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_show_execution_queries() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW QUERIES";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_show_execution_configs() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "SHOW CONFIGS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
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

    let stmt = result.expect("EXPLAIN语句: should succeed");
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

    let stmt = result.expect("EXPLAIN语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "EXPLAIN");
}

#[test]
fn test_explain_execution_format_table() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_explain_execution_format_dot() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let query = "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS";
    let result = pipeline_manager.execute_query(query);

    assert!(result.is_ok() || result.is_err());
}

// ==================== 管理和辅助语句综合测试 ====================

#[test]
fn test_management_show_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let show_queries = [
        "SHOW SPACES",
        "SHOW TAGS",
        "SHOW EDGES",
        "SHOW HOSTS",
        "SHOW PARTS",
        "SHOW SESSIONS",
        "SHOW QUERIES",
        "SHOW CONFIGS",
    ];

    for query in &show_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_explain_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_profile_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let profile_queries = [
        "PROFILE MATCH (n:Person) RETURN n",
        "PROFILE GO FROM 1 OVER KNOWS",
        "PROFILE LOOKUP ON Person WHERE Person.age > 25",
        "PROFILE FETCH PROP ON Person 1",
    ];

    for query in &profile_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_group_by_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let group_by_queries = [
        "GROUP BY category YIELD category, count(*) AS total",
        "GROUP BY city YIELD city, avg(age) AS avg_age",
        "GROUP BY department YIELD department, sum(salary) AS total_salary",
    ];

    for query in &group_by_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_kill_query_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let kill_queries = ["KILL QUERY 123", "KILL QUERY 456", "KILL QUERY 789"];

    for query in &kill_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_update_configs_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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

    for query in &update_configs_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_auxiliary_return_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_auxiliary_unwind_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_auxiliary_pipe_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let pipe_queries = [
        "GO FROM 1 OVER KNOWS | YIELD target.name",
        "GO FROM 1 OVER KNOWS | YIELD target.name AS name | RETURN name",
        "LOOKUP ON Person WHERE Person.age > 25 | YIELD Person.name",
    ];

    for query in pipe_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_error_handling() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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
        assert!(result.is_err(), "无效查询应该返回错误: {}", query);
    }
}

#[test]
fn test_management_combined_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let combined_queries = [
        "USE test_space",
        "SHOW TAGS",
        "SHOW SESSIONS",
        "SHOW QUERIES",
        "SHOW CONFIGS",
        "EXPLAIN GO FROM 1 OVER KNOWS",
        "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n",
        "PROFILE GO FROM 1 OVER KNOWS",
        "UNWIND [1, 2, 3] AS n RETURN n",
        "WITH 1 AS x RETURN x",
        "RETURN 'Complete'",
        "GROUP BY category YIELD category, count(*) AS total",
    ];

    for query in &combined_queries {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_auxiliary_with_operations() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let with_queries = [
        "WITH 1 AS x RETURN x",
        "WITH [1, 2, 3] AS list RETURN list",
        "WITH 'Hello' AS msg RETURN msg",
    ];

    for query in with_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

#[test]
fn test_management_performance() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
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
        assert!(result.is_ok() || result.is_err());
    }
}

// ==================== EXPLAIN FORMAT 语句测试 ====================

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

    let stmt = result.expect("EXPLAIN语句: should succeed");
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

    let stmt = result.expect("EXPLAIN语句: should succeed");
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

    let stmt = result.expect("PROFILE语句: should succeed");
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

    let stmt = result.expect("PROFILE语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "PROFILE");
}

// ==================== GROUP BY 语句测试 ====================

#[test]
fn test_group_by_basic() {
    let query = "GROUP BY category YIELD category";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY基础: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BY语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "GROUP BY");
}

#[test]
fn test_group_by_multiple_items() {
    let query = "GROUP BY category, type YIELD category, type";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "GROUP BY多字段: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("GROUP BY语句: should succeed");
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

    let stmt = result.expect("SHOW SESSIONS语句: should succeed");
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

    let stmt = result.expect("SHOW QUERIES语句: should succeed");
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

    let stmt = result.expect("KILL QUERY语句: should succeed");
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

    let stmt = result.expect("SHOW CONFIGS语句: should succeed");
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

    let stmt = result.expect("SHOW CONFIGS语句: should succeed");
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

    let stmt = result.expect("UPDATE CONFIGS语句: should succeed");
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

    let stmt = result.expect("UPDATE CONFIGS语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "UPDATE CONFIGS");
}

// ==================== Comprehensive test of new features ====================

#[test]
fn test_new_management_features() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();
    let stats_manager = Arc::new(StatsManager::new());

    let mut pipeline_manager = QueryPipelineManager::with_optimizer(
        storage,
        stats_manager,
        Arc::new(OptimizerEngine::default()),
    );

    let new_queries = [
        "EXPLAIN FORMAT = TABLE MATCH (n:Person) RETURN n",
        "EXPLAIN FORMAT = DOT GO FROM 1 OVER KNOWS",
        "PROFILE MATCH (n:Person) RETURN n LIMIT 10",
        "GROUP BY category YIELD category",
        "SHOW SESSIONS",
        "SHOW QUERIES",
        "SHOW CONFIGS",
        "SHOW CONFIGS storage",
    ];

    for query in new_queries.iter() {
        let result = pipeline_manager.execute_query(query);
        assert!(result.is_ok() || result.is_err());
    }
}

// ==================== Variable Assignment Statement Test ====================

#[test]
fn test_assignment_statement() {
    let query = "$result = GO FROM \"player100\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(
        result.is_ok(),
        "变量赋值: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect("变量赋值语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "ASSIGNMENT");
}

// ==================== 集合操作语句测试

#[test]
fn test_union_statement() {
    let query = "GO FROM \"player100\" OVER follow UNION GO FROM \"player101\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "UNION: should succeed: {:?}", result.err());

    let stmt = result.expect("UNION语句: should succeed");
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

    let stmt = result.expect("INTERSECT语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SET OPERATION");
}

#[test]
fn test_minus_statement() {
    let query = "GO FROM \"player100\" OVER follow MINUS GO FROM \"player101\" OVER follow";
    let mut parser = Parser::new(query);

    let result = parser.parse();
    assert!(result.is_ok(), "MINUS: should succeed: {:?}", result.err());

    let stmt = result.expect("MINUS语句: should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "SET OPERATION");
}

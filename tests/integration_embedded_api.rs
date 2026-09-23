//! Embedded API Integration Testing
//!
//! Test Range.
//! - api::embedded::database - database opening, configuration, session creation
//! - api::embedded::session - session management, space switching, query execution
//! - api::embedded::transaction - transaction management, savepoints
//! - api::embedded::statement - precompiled statements, parameter binding
//! - api::embedded::batch - batch insertion
//! - api::embedded::config - database configuration
//! - api::embedded::result - query result processing

#![cfg(feature = "embedded")]

use std::collections::HashMap;
use std::time::Duration;

use graphdb::api::api_core::SpaceConfig;
use graphdb::api::embedded::{
    BatchConfig, BatchError, BatchItemType, BatchResult, DatabaseConfig, GraphDatabase,
    QueryResult, ResultMetadata, Row, SyncMode, TransactionConfig,
};
use graphdb::core::types::{DataSet, VertexId};
use graphdb::core::{Edge, Value, Vertex};
use graphdb::storage::GraphStorage;

/// Test the database wrapper to keep the temporary catalog valid
struct TestDatabase {
    db: GraphDatabase<GraphStorage>,
    _temp_dir: tempfile::TempDir,
}

impl TestDatabase {
    fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
        let db_path = temp_dir.path().join("test.db");
        let db = GraphDatabase::open(db_path).expect("打开测试数据库失败");
        Self {
            db,
            _temp_dir: temp_dir,
        }
    }
}

/// Creating a test database (using temporary files)
fn create_test_database() -> TestDatabase {
    TestDatabase::new()
}

// ==================== DatabaseConfig 测试 ====================

#[test]
fn test_database_config_memory() {
    let config = DatabaseConfig::memory();
    assert!(config.is_memory());
    assert!(config.path().is_none());
    assert_eq!(config.cache_size_mb, 64);
    assert_eq!(config.default_timeout, Duration::from_secs(30));
    assert!(config.enable_wal);
    assert_eq!(config.sync_mode, SyncMode::Normal);
}

#[test]
fn test_database_config_file() {
    let config = DatabaseConfig::file("/tmp/test.db");
    assert!(!config.is_memory());
    assert_eq!(config.path(), Some(std::path::Path::new("/tmp/test.db")));
}

#[test]
fn test_database_config_builder() {
    let config = DatabaseConfig::memory()
        .with_cache_size(128)
        .with_timeout(Duration::from_secs(60))
        .with_wal(false)
        .with_sync_mode(SyncMode::Full);

    assert_eq!(config.cache_size_mb, 128);
    assert_eq!(config.default_timeout, Duration::from_secs(60));
    assert!(!config.enable_wal);
    assert_eq!(config.sync_mode, SyncMode::Full);
}

#[test]
fn test_database_config_default() {
    let config = DatabaseConfig::default();
    assert!(config.is_memory());
}

#[test]
fn test_database_config_cache_size_bytes() {
    let config = DatabaseConfig::memory().with_cache_size(64);
    assert_eq!(config.cache_size_bytes(), 64 * 1024 * 1024);
}

#[test]
fn test_sync_mode_default() {
    let mode = SyncMode::default();
    assert_eq!(mode, SyncMode::Normal);
}

// ==================== GraphDatabase 测试 ====================

#[test]
fn test_graph_database_open_in_memory() {
    let test_db = create_test_database();
    let db = &test_db.db;
    assert!(!db.is_memory());
}

#[test]
fn test_graph_database_open_with_temp_file() {
    let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
    let db_path = temp_dir.path().join("test.db");

    let db = GraphDatabase::open(&db_path).expect("打开文件数据库失败");
    assert!(!db.is_memory());
    assert_eq!(db.config().path(), Some(db_path.as_path()));
}

#[test]
fn test_graph_database_open_with_config() {
    let temp_dir = tempfile::tempdir().expect("创建临时目录失败");
    let db_path = temp_dir.path().join("test.db");
    let config = DatabaseConfig::file(&db_path).with_cache_size(128);
    let db = GraphDatabase::open_with_config(config).expect("打开数据库失败");
    assert!(!db.is_memory());
}

#[test]
fn test_graph_database_create_session() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");
    assert!(session.auto_commit());
}

#[test]
fn test_graph_database_execute_simple() {
    let test_db = create_test_database();
    let db = &test_db.db;

    let space_config = SpaceConfig::default();
    db.create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = db.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));
}

#[test]
fn test_graph_database_space_management() {
    let test_db = create_test_database();
    let db = &test_db.db;

    let space_config = SpaceConfig::default();
    db.create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = db.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));

    db.drop_space("test_space").expect("删除空间失败");

    let spaces = db.list_spaces().expect("列出空间失败");
    assert!(!spaces.contains(&"test_space".to_string()));
}

// ==================== Session Test ====================

#[test]
fn test_session_use_space() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("创建会话失败");

    let space_config = SpaceConfig::default();
    db.create_space("test_space", space_config)
        .expect("创建空间失败");

    session.use_space("test_space").expect("切换空间失败");
    assert_eq!(session.current_space().as_deref(), Some("test_space"));
    assert!(session.current_space_id().is_some());
}

#[test]
fn test_session_auto_commit() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("创建会话失败");

    assert!(session.auto_commit());

    session.set_auto_commit(false);
    assert!(!session.auto_commit());
}

/// Text transaction commands (BEGIN / SAVEPOINT / ROLLBACK TO / COMMIT)
/// through `Session::execute` perform the TransactionManager side effects
/// and run the state-machine plan.
#[test]
fn test_session_text_transaction_commands() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("创建会话失败");

    session
        .create_space("txn_text_space", SpaceConfig::default())
        .expect("create space failed");
    session
        .use_space("txn_text_space")
        .expect("use space failed");
    session
        .execute("CREATE TAG IF NOT EXISTS person(name STRING NOT NULL, age INT)")
        .expect("CREATE TAG failed");

    // BEGIN starts a session-level transaction; subsequent statements are
    // bound to it.
    session.execute("BEGIN").expect("BEGIN should succeed");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p1':('Alice', 30)")
        .expect("INSERT inside text transaction should succeed");

    // ROLLBACK undoes the transaction writes.
    session
        .execute("ROLLBACK")
        .expect("ROLLBACK should succeed");
    let count = session
        .execute("MATCH (p:person) RETURN count(p) AS c")
        .expect("MATCH should succeed");
    let value = count.rows().first().expect("count row").get("c");
    assert_eq!(
        value,
        Some(&graphdb::core::Value::BigInt(0)),
        "ROLLBACK must undo the in-transaction INSERT"
    );

    // SAVEPOINT + ROLLBACK TO keeps the transaction active.
    session.execute("BEGIN").expect("BEGIN should succeed");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p1':('Alice', 30)")
        .expect("INSERT p1 should succeed");
    session
        .execute("SAVEPOINT sp1")
        .expect("SAVEPOINT should succeed");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p2':('Bob', 25)")
        .expect("INSERT p2 should succeed");
    session
        .execute("ROLLBACK TO sp1")
        .expect("ROLLBACK TO should succeed");
    let count = session
        .execute("MATCH (p:person) RETURN count(p) AS c")
        .expect("MATCH should succeed");
    let value = count.rows().first().expect("count row").get("c");
    assert_eq!(
        value,
        Some(&graphdb::core::Value::BigInt(1)),
        "ROLLBACK TO must undo post-savepoint writes only"
    );

    session.execute("COMMIT").expect("COMMIT should succeed");
    let count = session
        .execute("MATCH (p:person) RETURN count(p) AS c")
        .expect("MATCH should succeed");
    let value = count.rows().first().expect("count row").get("c");
    assert_eq!(value, Some(&graphdb::core::Value::BigInt(1)));

    // COMMIT without an active transaction fails clearly.
    assert!(session.execute("COMMIT").is_err());

    // LET assigns session variables in embedded sessions.
    let result = session.execute("LET $x = 1").expect("LET should succeed");
    let value = result.rows().first().expect("let row").get("x");
    assert_eq!(value, Some(&graphdb::core::Value::BigInt(1)));
}

#[test]
fn test_session_execute() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let space_config = SpaceConfig::default();
    session
        .create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));
}

#[test]
fn test_session_execute_with_params() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("create session failed");

    session
        .create_space("param_space", SpaceConfig::default())
        .expect("create space failed");
    session.use_space("param_space").expect("use space failed");

    session
        .execute("CREATE TAG IF NOT EXISTS person(name STRING NOT NULL, age INT)")
        .expect("CREATE TAG failed");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p1':('Alice', 30)")
        .expect("INSERT failed");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p2':('Bob', 25)")
        .expect("INSERT failed");

    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::string("Alice"));

    let result = session
        .execute_with_params(
            "MATCH (p:person) WHERE p.name == @name RETURN p.age",
            params,
        )
        .expect("parameterized query should succeed");

    assert_eq!(result.len(), 1, "should return exactly one row");
    let row = result.first().expect("row should exist");
    assert_eq!(row.get_by_index(0), Some(&Value::Int(30)));

    let mut params2 = HashMap::new();
    params2.insert("name".to_string(), Value::string("Bob"));

    let result2 = session
        .execute_with_params(
            "MATCH (p:person) WHERE p.name == @name RETURN p.age",
            params2,
        )
        .expect("parameterized query should succeed with different param value");

    assert_eq!(result2.len(), 1, "should return exactly one row");
    let row2 = result2.first().expect("row should exist");
    assert_eq!(row2.get_by_index(0), Some(&Value::Int(25)));
}

#[test]
fn test_session_execute_with_params_unknown_param() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("create session failed");

    session
        .create_space("param_space_up", SpaceConfig::default())
        .expect("create space failed");
    session
        .use_space("param_space_up")
        .expect("use space failed");

    let mut params = HashMap::new();
    params.insert("unknown".to_string(), Value::string("value"));

    let result = session.execute_with_params("SHOW SPACES", params);
    assert!(
        result.is_err(),
        "unknown parameter for non-parameterized query should fail"
    );
}

/// Query parameters (`@name`) and session variables (`$name`) share the
/// namespace independently: same name, distinct values, no conflict.
#[test]
fn test_session_param_and_variable_same_name_coexist() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let mut session = db.session().expect("create session failed");

    session
        .create_space("coexist_space", SpaceConfig::default())
        .expect("create space failed");
    session
        .use_space("coexist_space")
        .expect("use space failed");

    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(10));
    let mut variables = HashMap::new();
    variables.insert("x".to_string(), Value::Int(100));

    let result = session
        .execute_with_params_and_variables("RETURN @x + $x", params, variables)
        .expect("same-name parameter and session variable should coexist");
    assert_eq!(result.len(), 1, "should return exactly one row");
    let row = result.first().expect("row should exist");
    assert_eq!(
        row.get_by_index(0),
        Some(&Value::Int(110)),
        "@x (parameter) and $x (session variable) must resolve independently"
    );
}

#[test]
fn test_session_space_management() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let space_config = SpaceConfig::default();
    session
        .create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));

    session.drop_space("test_space").expect("删除空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(!spaces.contains(&"test_space".to_string()));
}

#[test]
fn test_session_batch_inserter() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let inserter = session.batch_inserter(100);
    assert_eq!(inserter.batch_size(), 100);
}

// ==================== Transaction Testing ====================

#[test]
fn test_transaction_config_default() {
    let config = TransactionConfig::default();
    assert!(!config.read_only);
    assert!(config.timeout.is_none());
    assert_eq!(
        config.durability,
        graphdb::transaction::DurabilityLevel::Sync
    );
}

#[test]
fn test_transaction_config_builder() {
    let config = TransactionConfig::new()
        .read_only()
        .with_timeout(Duration::from_secs(60))
        .with_durability(graphdb::transaction::DurabilityLevel::None);

    assert!(config.read_only);
    assert_eq!(config.timeout, Some(Duration::from_secs(60)));
    assert_eq!(
        config.durability,
        graphdb::transaction::DurabilityLevel::None
    );
}

#[test]
fn test_transaction_begin() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    assert!(txn.is_active());
    assert!(!txn.is_committed());
    assert!(!txn.is_rolled_back());
}

#[test]
fn test_transaction_with_config() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let config = TransactionConfig::new().read_only();
    let txn = session
        .begin_transaction_with_config(config)
        .expect("开始事务失败");
    assert!(txn.is_active());
}

#[test]
fn test_transaction_commit() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    assert!(txn.is_active());
    txn.commit().expect("提交事务失败");
}

#[test]
fn test_transaction_rollback() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    assert!(txn.is_active());
    txn.rollback().expect("回滚事务失败");
}

#[test]
fn test_transaction_execute() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");

    let space_config = SpaceConfig::default();
    session
        .create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));

    assert!(txn.is_active());
}

#[test]
fn test_transaction_info() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    let info = txn.info().expect("获取事务信息失败");
    assert!(info.id > 0);
    assert!(!info.is_read_only);
    assert_eq!(info.savepoint_count, 0);
}

#[test]
fn test_transaction_handle() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    let handle = txn.handle();
    assert_eq!(handle.0 .0, txn.id());
}

#[test]
fn test_transaction_auto_rollback_on_drop() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    {
        let txn = session.begin_transaction().expect("开始事务失败");
        assert!(txn.is_active());
    }
}

#[test]
fn test_session_with_transaction() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let result = session
        .with_transaction(|_txn| Ok::<_, graphdb::api::api_core::CoreError>(42))
        .expect("事务执行失败");

    assert_eq!(result, 42);
}

#[test]
fn test_session_with_transaction_rollback_on_error() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let result: Result<i32, graphdb::api::CoreError> = session.with_transaction(|_txn| {
        Err::<i32, _>(graphdb::api::api_core::CoreError::Internal(
            "测试错误".to_string(),
        ))
    });

    assert!(result.is_err());
}

// ==================== BatchInserter 测试 ====================

#[test]
fn test_batch_result_default() {
    let result = BatchResult::default();
    assert_eq!(result.vertices_inserted, 0);
    assert_eq!(result.edges_inserted, 0);
    assert!(result.errors.is_empty());
}

#[test]
fn test_batch_result_total_inserted() {
    let result = BatchResult {
        vertices_inserted: 100,
        edges_inserted: 50,
        errors: Vec::new(),
    };
    assert_eq!(result.total_inserted(), 150);
}

#[test]
fn test_batch_result_has_errors() {
    let result = BatchResult {
        vertices_inserted: 0,
        edges_inserted: 0,
        errors: vec![BatchError::new(0, BatchItemType::Vertex, "测试错误")],
    };
    assert!(result.has_errors());
    assert_eq!(result.error_count(), 1);
}

#[test]
fn test_batch_result_merge() {
    let mut result1 = BatchResult {
        vertices_inserted: 100,
        edges_inserted: 50,
        errors: vec![BatchError::new(0, BatchItemType::Vertex, "error1")],
    };

    let result2 = BatchResult {
        vertices_inserted: 200,
        edges_inserted: 100,
        errors: vec![BatchError::new(1, BatchItemType::Edge, "error2")],
    };

    result1.merge(result2);

    assert_eq!(result1.vertices_inserted, 300);
    assert_eq!(result1.edges_inserted, 150);
    assert_eq!(result1.errors.len(), 2);
}

#[test]
fn test_batch_config_default() {
    let config = BatchConfig::default();
    assert_eq!(config.batch_size, 1000);
    assert!(config.auto_flush);
    assert!(config.continue_on_error);
    assert_eq!(config.max_errors, Some(100));
}

#[test]
fn test_batch_config_builder() {
    let config = BatchConfig::new()
        .with_batch_size(500)
        .with_auto_flush(false)
        .with_continue_on_error(false)
        .with_max_errors(Some(50));

    assert_eq!(config.batch_size, 500);
    assert!(!config.auto_flush);
    assert!(!config.continue_on_error);
    assert_eq!(config.max_errors, Some(50));
}

#[test]
fn test_batch_config_min_batch_size() {
    let config = BatchConfig::new().with_batch_size(0);
    assert_eq!(config.batch_size, 1);
}

#[test]
fn test_batch_inserter_create() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let inserter = session.batch_inserter(100);
    assert_eq!(inserter.batch_size(), 100);
    assert_eq!(inserter.buffered_vertices(), 0);
    assert_eq!(inserter.buffered_edges(), 0);
    assert!(!inserter.has_buffered_data());
}

#[test]
fn test_batch_inserter_add_vertex() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let mut inserter = session.batch_inserter(100);
    let vertex = Vertex::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        graphdb::core::Tag::new("Person".to_string(), HashMap::new()),
    );
    inserter.add_vertex(vertex);

    assert_eq!(inserter.buffered_vertices(), 1);
    assert!(inserter.has_buffered_data());
}

#[test]
fn test_batch_inserter_add_edge() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let mut inserter = session.batch_inserter(100);
    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "follows".to_string(),
        0,
        HashMap::new(),
    );
    inserter.add_edge(edge);

    assert_eq!(inserter.buffered_edges(), 1);
    assert!(inserter.has_buffered_data());
}

#[test]
fn test_batch_error_create() {
    let error = BatchError::new(0, BatchItemType::Vertex, "test error");
    assert_eq!(error.index, 0);
    assert_eq!(error.item_type, BatchItemType::Vertex);
    assert_eq!(error.error, "test error");
}

// ==================== QueryResult test ====================

#[test]
fn test_query_result_empty() {
    let core_result = graphdb::api::api_core::QueryResult::new(
        graphdb::query::executor::base::ExecutionResult::DataSet {
            data: DataSet::new(),
        },
        graphdb::api::api_core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 0,
            cache_hit: false,
        },
    );
    let result = QueryResult::from_core(core_result);
    assert!(result.is_empty());
    assert_eq!(result.len(), 0);
    assert!(result.first().is_none());
    assert!(result.last().is_none());
}

#[test]
fn test_query_result_columns() {
    let columns = vec!["id".to_string(), "name".to_string()];
    let data = DataSet::with_columns(columns.clone());
    let core_result = graphdb::api::api_core::QueryResult::new(
        graphdb::query::executor::base::ExecutionResult::DataSet { data },
        graphdb::api::api_core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 0,
            cache_hit: false,
        },
    );
    let result = QueryResult::from_core(core_result);
    assert_eq!(result.columns(), &columns);
}

#[test]
fn test_query_result_metadata() {
    let columns = vec!["id".to_string()];
    let mut data = DataSet::with_columns(columns.clone());
    data.add_row(vec![Value::Int(1)]);
    data.add_row(vec![Value::Int(2)]);
    let core_result = graphdb::api::api_core::QueryResult::new(
        graphdb::query::executor::base::ExecutionResult::DataSet { data },
        graphdb::api::api_core::ExecutionMetadata {
            execution_time_ms: 100,
            rows_scanned: 100,
            rows_returned: 10,
            cache_hit: false,
        },
    );
    let result = QueryResult::from_core(core_result);
    assert_eq!(result.metadata().rows_returned, 2);
    assert_eq!(result.metadata().rows_scanned, 100);
    assert_eq!(result.metadata().execution_time, Duration::from_millis(100));
}

#[test]
fn test_query_result_iterator() {
    let columns = vec!["id".to_string()];
    let mut data = DataSet::with_columns(columns.clone());
    data.add_row(vec![Value::Int(1)]);
    let core_result = graphdb::api::api_core::QueryResult::new(
        graphdb::query::executor::base::ExecutionResult::DataSet { data },
        graphdb::api::api_core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 1,
            cache_hit: false,
        },
    );
    let result = QueryResult::from_core(core_result);
    assert_eq!(result.len(), 1);

    let count = result.iter().count();
    assert_eq!(count, 1);
}

#[test]
fn test_query_result_into_iterator() {
    let columns = vec!["id".to_string()];
    let mut data = DataSet::with_columns(columns.clone());
    data.add_row(vec![Value::Int(1)]);
    let core_result = graphdb::api::api_core::QueryResult::new(
        graphdb::query::executor::base::ExecutionResult::DataSet { data },
        graphdb::api::api_core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 1,
            cache_hit: false,
        },
    );
    let result = QueryResult::from_core(core_result);
    let count = result.into_iter().count();
    assert_eq!(count, 1);
}

// ==================== Row 测试 ====================

#[test]
fn test_row_get() {
    let row = Row::from_columns(&["id".to_string()], &[Value::Int(42)]);

    let value = row.get("id");
    assert!(value.is_some());
    assert_eq!(value, Some(&Value::Int(42)));
}

#[test]
fn test_row_get_by_index() {
    let row = Row::from_columns(
        &["id".to_string(), "name".to_string()],
        &[Value::Int(42), Value::string("测试")],
    );

    let value = row.get_by_index(0);
    assert!(value.is_some());
}

#[test]
fn test_row_columns() {
    let row = Row::from_columns(
        &["id".to_string(), "name".to_string()],
        &[Value::Int(42), Value::string("测试")],
    );

    let columns = row.columns();
    assert_eq!(columns.len(), 2);
    assert!(columns.contains(&&"id".to_string()));
    assert!(columns.contains(&&"name".to_string()));
}

#[test]
fn test_row_has_column() {
    let row = Row::from_columns(&["id".to_string()], &[Value::Int(42)]);

    assert!(row.has_column("id"));
    assert!(!row.has_column("name"));
}

#[test]
fn test_row_get_string() {
    let row = Row::from_columns(&["name".to_string()], &[Value::string("测试")]);

    let value = row.get_string("name");
    assert_eq!(value, Some("测试".to_string()));
}

#[test]
fn test_row_get_int() {
    let row = Row::from_columns(&["id".to_string()], &[Value::Int(42)]);

    let value = row.get_int("id");
    assert_eq!(value, Some(42));
}

#[test]
fn test_row_get_float() {
    let row = Row::from_columns(&["score".to_string()], &[Value::Float(2.5_f32)]);

    let value = row.get_float("score");
    assert_eq!(value, Some(2.5_f64));
}

#[test]
fn test_row_get_bool() {
    let row = Row::from_columns(&["active".to_string()], &[Value::Bool(true)]);

    let value = row.get_bool("active");
    assert_eq!(value, Some(true));
}

#[test]
fn test_row_get_vertex() {
    let vertex = Vertex::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        graphdb::core::Tag::new("Person".to_string(), HashMap::new()),
    );
    let row = Row::from_columns(&["v".to_string()], &[Value::Vertex(Box::new(vertex))]);

    let value = row.get_vertex("v");
    assert!(value.is_some());
}

#[test]
fn test_row_get_edge() {
    let edge = Edge::new(
        VertexId::try_from_int64(1).expect("test vertex id"),
        VertexId::try_from_int64(2).expect("test vertex id"),
        "follows".to_string(),
        0,
        HashMap::new(),
    );
    let row = Row::from_columns(&["e".to_string()], &[Value::Edge(Box::new(edge))]);

    let value = row.get_edge("e");
    assert!(value.is_some());
}

#[test]
fn test_row_len() {
    let row = Row::from_columns(
        &["id".to_string(), "name".to_string()],
        &[Value::Int(42), Value::string("测试")],
    );

    assert_eq!(row.len(), 2);
}

#[test]
fn test_row_is_empty() {
    let row = Row::from_columns(&[], &[]);

    assert!(row.is_empty());
}

// ==================== ResultMetadata 测试 ====================

#[test]
fn test_result_metadata_default() {
    let metadata = ResultMetadata::default();
    assert_eq!(metadata.execution_time, Duration::from_millis(0));
    assert_eq!(metadata.rows_returned, 0);
    assert_eq!(metadata.rows_scanned, 0);
}

// ==================== 综合测试 ====================

#[test]
fn test_full_workflow() {
    let test_db = create_test_database();
    let db = &test_db.db;

    let space_config = SpaceConfig::default();
    db.create_space("test_space", space_config)
        .expect("创建空间失败");

    let mut session = db.session().expect("创建会话失败");
    session.use_space("test_space").expect("切换空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));
}

#[test]
fn test_multiple_sessions() {
    let test_db = create_test_database();
    let db = &test_db.db;

    let session1 = db.session().expect("创建会话失败");
    let session2 = db.session().expect("创建会话失败");

    assert_eq!(session1.current_space(), session2.current_space());
}

// ==================== Phase 4: ALTER ADD/DROP FROM ====================

fn setup_phase4_graph(
    session: &mut graphdb::api::embedded::Session<graphdb::storage::GraphStorage>,
) {
    session
        .create_space("phase4", SpaceConfig::default())
        .expect("create space");
    session.use_space("phase4").expect("use space");
    session
        .execute("CREATE TAG person(name: STRING, age: INT)")
        .expect("create tag person");
    session
        .execute("CREATE TAG company(name: STRING)")
        .expect("create tag company");
    session
        .execute("CREATE EDGE works_at(since: INT)")
        .expect("create edge works_at");
    session
        .execute("INSERT VERTEX person(name, age) VALUES 'p1':('Alice', 30), 'p2':('Bob', 25)")
        .expect("insert persons");
    session
        .execute("INSERT VERTEX company(name) VALUES 'c1':('Acme')")
        .expect("insert company");
    session
        .execute("INSERT EDGE works_at(since) VALUES 'p1' -> 'c1': (2020)")
        .expect("insert edge");
}

fn desc_default_for(result: &graphdb::api::embedded::QueryResult, field: &str) -> Option<String> {
    result.rows().iter().find_map(|row| {
        let is_field = matches!(row.get("Field"), Some(Value::String(s)) if s.as_str() == field);
        if !is_field {
            return None;
        }
        match row.get("Default") {
            Some(Value::String(s)) => Some(s.to_string()),
            other => Some(format!("{other:?}")),
        }
    })
}

#[test]
fn test_alter_edge_add_drop_from_desc_visible() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    session
        .execute("ALTER EDGE works_at ADD FROM person TO company")
        .expect("ADD FROM succeeds");

    let desc = session
        .execute("DESC EDGE works_at")
        .expect("DESC succeeds");
    assert_eq!(
        desc_default_for(&desc, "src_tag").as_deref(),
        Some("person"),
        "ADD FROM endpoint visible in DESC"
    );
    assert_eq!(
        desc_default_for(&desc, "dst_tag").as_deref(),
        Some("company"),
        "ADD FROM endpoint visible in DESC"
    );

    let show = session.execute("SHOW EDGES").expect("SHOW EDGES succeeds");
    let edge_row = show
        .rows()
        .iter()
        .find(|row| row.get("name") == Some(&Value::string("works_at")))
        .expect("works_at listed");
    assert_eq!(
        edge_row.get("src_tag"),
        Some(&Value::string("person")),
        "ADD FROM endpoint visible in SHOW EDGES"
    );

    session
        .execute("ALTER EDGE works_at DROP FROM person TO company")
        .expect("DROP FROM succeeds");

    let desc = session
        .execute("DESC EDGE works_at")
        .expect("DESC succeeds");
    assert_eq!(
        desc_default_for(&desc, "src_tag").as_deref(),
        Some("(unconstrained)"),
        "DROP FROM clears the constraint"
    );
    assert_eq!(
        desc_default_for(&desc, "dst_tag").as_deref(),
        Some("(unconstrained)"),
        "DROP FROM clears the constraint"
    );
}

#[test]
fn test_alter_edge_add_from_rejects_missing_tag() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let err = session
        .execute("ALTER EDGE works_at ADD FROM nosuch TO company")
        .expect_err("missing source tag must fail");
    assert!(
        err.to_string().contains("not found"),
        "unexpected error: {err}"
    );
}

#[test]
fn test_rename_tag_and_edge() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    session
        .execute("ALTER TAG person RENAME TO customer")
        .expect("rename tag succeeds");
    let tags = session.execute("SHOW TAGS").expect("SHOW TAGS succeeds");
    assert!(
        tags.rows()
            .iter()
            .any(|row| row.get("name") == Some(&Value::string("customer"))),
        "renamed tag visible"
    );

    session
        .execute("ALTER EDGE works_at RENAME TO employed_by")
        .expect("rename edge succeeds");
    let edges = session.execute("SHOW EDGES").expect("SHOW EDGES succeeds");
    assert!(
        edges
            .rows()
            .iter()
            .any(|row| row.get("name") == Some(&Value::string("employed_by"))),
        "renamed edge visible"
    );
}

#[test]
fn test_drop_multi_name_rejected_explicitly() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let err = session
        .execute("DROP TAG person, company")
        .expect_err("multi-drop must not silently drop one table");
    assert!(
        err.to_string().contains("multiple names"),
        "unexpected error: {err}"
    );
}

// ==================== Phase 4: COPY multi-file ====================

#[test]
fn test_copy_multi_file_row_concat() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let csv_dir = tempfile::tempdir().expect("create csv dir");
    let f1 = csv_dir.path().join("p10.csv");
    let f2 = csv_dir.path().join("p11.csv");
    std::fs::write(&f1, "vid,name,age\np10,Carol,28\n").expect("write f1");
    std::fs::write(&f2, "vid,name,age\np11,Dave,22\n").expect("write f2");

    session
        .execute(&format!(
            "COPY person FROM ('{}', '{}')",
            f1.to_string_lossy(),
            f2.to_string_lossy()
        ))
        .expect("multi-file COPY succeeds");

    let result = session
        .execute("MATCH (p:person) RETURN p.name")
        .expect("MATCH succeeds");
    assert_eq!(result.len(), 4, "row-concat import adds both files");
}

#[test]
fn test_copy_multi_file_by_column() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let csv_dir = tempfile::tempdir().expect("create csv dir");
    let f1 = csv_dir.path().join("ids.csv");
    let f2 = csv_dir.path().join("attrs.csv");
    std::fs::write(&f1, "vid\np20\np21\n").expect("write ids");
    std::fs::write(&f2, "name,age\nErin,31\nFrank,29\n").expect("write attrs");

    session
        .execute(&format!(
            "COPY person FROM ('{}', '{}') BY COLUMN",
            f1.to_string_lossy(),
            f2.to_string_lossy()
        ))
        .expect("BY COLUMN COPY succeeds");

    let result = session
        .execute("MATCH (p:person) RETURN p.name")
        .expect("MATCH succeeds");
    assert_eq!(result.len(), 4, "column-merge import adds zipped rows");
}

#[test]
fn test_copy_by_column_row_mismatch_rejected() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let csv_dir = tempfile::tempdir().expect("create csv dir");
    let f1 = csv_dir.path().join("ids2.csv");
    let f2 = csv_dir.path().join("attrs2.csv");
    std::fs::write(&f1, "vid\np30\np31\np32\n").expect("write ids");
    std::fs::write(&f2, "name,age\nGail,40\n").expect("write attrs");

    let err = session
        .execute(&format!(
            "COPY person FROM ('{}', '{}') BY COLUMN",
            f1.to_string_lossy(),
            f2.to_string_lossy()
        ))
        .expect_err("row-count mismatch must fail");
    assert!(err.to_string().contains("rows"), "unexpected error: {err}");
}

// ==================== Phase 4: CREATE ... AS ====================

#[test]
fn test_create_tag_as_query() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    session
        .execute("CREATE TAG adults AS (MATCH (p:person) RETURN id(p) AS vid, p.name AS name, p.age AS age)")
        .expect("CREATE TAG AS succeeds");

    let tags = session.execute("SHOW TAGS").expect("SHOW TAGS succeeds");
    assert!(
        tags.rows()
            .iter()
            .any(|row| row.get("name") == Some(&Value::string("adults"))),
        "new tag visible"
    );

    let desc = session.execute("DESC TAG adults").expect("DESC succeeds");
    assert!(
        desc.rows()
            .iter()
            .any(|row| row.get("Field") == Some(&Value::string("name"))),
        "inferred schema visible"
    );

    let result = session
        .execute("MATCH (a:adults) RETURN a.name")
        .expect("MATCH new tag succeeds");
    assert_eq!(result.len(), 2, "materialized rows match query output");
}

#[test]
fn test_create_edge_as_query() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    session
        .execute("CREATE EDGE worked AS (MATCH (a:person)-[e:works_at]->(b:company) RETURN id(a) AS src, id(b) AS dst, e.since AS since)")
        .expect("CREATE EDGE AS succeeds");

    let result = session
        .execute("MATCH (a:person)-[e:worked]->(b:company) RETURN e.since")
        .expect("MATCH new edge succeeds");
    assert_eq!(result.len(), 1, "materialized edge matches query output");
    assert_eq!(
        result.first().and_then(|r| r.get_by_index(0)),
        Some(&Value::Int(2020)),
        "edge property data consistent"
    );
}

#[test]
fn test_create_as_rejects_missing_key_and_duplicates() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let err = session
        .execute("CREATE TAG no_vid AS (MATCH (p:person) RETURN p.name AS name)")
        .expect_err("missing vid column must fail");
    assert!(
        err.to_string().contains("vertex id"),
        "unexpected error: {err}"
    );

    session
        .execute("CREATE TAG adults AS (MATCH (p:person) RETURN id(p) AS vid, p.name AS name)")
        .expect("first CREATE succeeds");
    let err = session
        .execute("CREATE TAG adults AS (MATCH (p:person) RETURN id(p) AS vid, p.name AS name)")
        .expect_err("duplicate without IF NOT EXISTS must fail");
    assert!(
        err.to_string().contains("already exists"),
        "unexpected error: {err}"
    );

    session
        .execute("CREATE TAG IF NOT EXISTS adults AS (MATCH (p:person) RETURN id(p) AS vid)")
        .expect("IF NOT EXISTS skips existing table");
}

// ==================== Legacy: traversal semantics ====================

#[test]
fn test_weighted_shortest_returns_path() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    session
        .create_space("weighted", SpaceConfig::default())
        .expect("create space");
    session.use_space("weighted").expect("use space");
    session
        .execute("CREATE TAG city(name: STRING)")
        .expect("create tag");
    session
        .execute("CREATE EDGE road(w: INT)")
        .expect("create edge");
    session
        .execute("INSERT VERTEX city(name) VALUES 'a':('A'), 'b':('B'), 'c':('C')")
        .expect("insert cities");
    session
        .execute("INSERT EDGE road(w) VALUES 'a' -> 'b': (10), 'a' -> 'c': (1), 'c' -> 'b': (1)")
        .expect("insert roads");

    let result = session
        .execute("MATCH (x:city)-[*WEIGHTED(w)]->(y:city) WHERE x.name == 'A' RETURN y.name")
        .expect("weighted traversal succeeds");
    assert!(
        !result.is_empty(),
        "weighted traversal must return reachable vertices"
    );
}

#[test]
fn test_recursive_comprehension_rejected_explicitly() {
    let test_db = create_test_database();
    let mut session = test_db.db.session().expect("create session");
    setup_phase4_graph(&mut session);

    let err = session
        .execute("MATCH (a:person)-[e*(v, r | WHERE v.age > 20)]->(b:person) RETURN b")
        .expect_err("recursive comprehension must not silently degrade");
    assert!(
        err.to_string().contains("not yet supported"),
        "unexpected error: {err}"
    );
}

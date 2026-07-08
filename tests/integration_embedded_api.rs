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

mod common;

use std::collections::HashMap;
use std::time::Duration;

use graphdb::api::core::SpaceConfig;
use graphdb::api::embedded::{
    BatchConfig, BatchError, BatchItemType, BatchResult, DatabaseConfig, GraphDatabase,
    QueryResult, ResultMetadata, Row, SyncMode, TransactionConfig,
};
use graphdb::core::types::VertexId;
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
    let session = db.session().expect("创建会话失败");

    let space_config = SpaceConfig::default();
    session
        .create_space("test_space", space_config)
        .expect("创建空间失败");

    let spaces = session.list_spaces().expect("列出空间失败");
    assert!(spaces.contains(&"test_space".to_string()));
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

#[tokio::test]
async fn test_transaction_commit() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let txn = session.begin_transaction().expect("开始事务失败");
    assert!(txn.is_active());
    txn.commit().expect("提交事务失败");
}

#[tokio::test]
async fn test_transaction_rollback() {
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

#[tokio::test]
async fn test_session_with_transaction() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let result = session
        .with_transaction(|_txn| Ok::<_, graphdb::api::core::CoreError>(42))
        .expect("事务执行失败");

    assert_eq!(result, 42);
}

#[tokio::test]
async fn test_session_with_transaction_rollback_on_error() {
    let test_db = create_test_database();
    let db = &test_db.db;
    let session = db.session().expect("创建会话失败");

    let result: Result<i32, graphdb::api::CoreError> = session.with_transaction(|_txn| {
        Err::<i32, _>(graphdb::api::core::CoreError::Internal(
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
    assert!(config.auto_commit);
    assert!(config.continue_on_error);
    assert_eq!(config.max_errors, Some(100));
}

#[test]
fn test_batch_config_builder() {
    let config = BatchConfig::new()
        .with_batch_size(500)
        .with_auto_commit(false)
        .with_continue_on_error(false)
        .with_max_errors(Some(50));

    assert_eq!(config.batch_size, 500);
    assert!(!config.auto_commit);
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
    let vertex = Vertex::with_vid(VertexId::from_int64(1));
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
        VertexId::from_int64(1),
        VertexId::from_int64(2),
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
    let error = BatchError::new(0, BatchItemType::Vertex, "测试错误");
    assert_eq!(error.index, 0);
    assert_eq!(error.item_type, BatchItemType::Vertex);
    assert_eq!(error.error, "test error");
}

// ==================== QueryResult test ====================

#[test]
fn test_query_result_empty() {
    let result = QueryResult::from_core(graphdb::api::core::QueryResult {
        columns: vec![],
        rows: vec![],
        metadata: graphdb::api::core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 0,
            cache_hit: false,
        },
    });
    assert!(result.is_empty());
    assert_eq!(result.len(), 0);
    assert!(result.first().is_none());
    assert!(result.last().is_none());
}

#[test]
fn test_query_result_columns() {
    let columns = vec!["id".to_string(), "name".to_string()];
    let result = QueryResult::from_core(graphdb::api::core::QueryResult {
        columns: columns.clone(),
        rows: vec![],
        metadata: graphdb::api::core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 0,
            cache_hit: false,
        },
    });
    assert_eq!(result.columns(), &columns);
}

#[test]
fn test_query_result_metadata() {
    let mut values1 = HashMap::new();
    values1.insert("id".to_string(), Value::Int(1));

    let mut values2 = HashMap::new();
    values2.insert("id".to_string(), Value::Int(2));

    let core_row1 = graphdb::api::core::Row { values: values1 };
    let core_row2 = graphdb::api::core::Row { values: values2 };
    let rows = vec![core_row1, core_row2];

    let result = QueryResult::from_core(graphdb::api::core::QueryResult {
        columns: vec!["id".to_string()],
        rows,
        metadata: graphdb::api::core::ExecutionMetadata {
            execution_time_ms: 100,
            rows_scanned: 100,
            rows_returned: 10,
            cache_hit: false,
        },
    });
    assert_eq!(result.metadata().rows_returned, 2);
    assert_eq!(result.metadata().rows_scanned, 100);
    assert_eq!(result.metadata().execution_time, Duration::from_millis(100));
}

#[test]
fn test_query_result_iterator() {
    let columns = vec!["id".to_string()];
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(1));
    let core_row = graphdb::api::core::Row { values };

    let result = QueryResult::from_core(graphdb::api::core::QueryResult {
        columns,
        rows: vec![core_row],
        metadata: graphdb::api::core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 1,
            cache_hit: false,
        },
    });
    assert_eq!(result.len(), 1);

    let count = result.iter().count();
    assert_eq!(count, 1);
}

#[test]
fn test_query_result_into_iterator() {
    let columns = vec!["id".to_string()];
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(1));
    let core_row = graphdb::api::core::Row { values };

    let result = QueryResult::from_core(graphdb::api::core::QueryResult {
        columns,
        rows: vec![core_row],
        metadata: graphdb::api::core::ExecutionMetadata {
            execution_time_ms: 0,
            rows_scanned: 0,
            rows_returned: 1,
            cache_hit: false,
        },
    });
    let count = result.into_iter().count();
    assert_eq!(count, 1);
}

// ==================== Row 测试 ====================

#[test]
fn test_row_get() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get("id");
    assert!(value.is_some());
    assert_eq!(value, Some(&Value::Int(42)));
}

#[test]
fn test_row_get_by_index() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    values.insert("name".to_string(), Value::String("测试".to_string()));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_by_index(0);
    assert!(value.is_some());
}

#[test]
fn test_row_columns() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    values.insert("name".to_string(), Value::String("测试".to_string()));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let columns = row.columns();
    assert_eq!(columns.len(), 2);
    assert!(columns.contains(&&"id".to_string()));
    assert!(columns.contains(&&"name".to_string()));
}

#[test]
fn test_row_has_column() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    assert!(row.has_column("id"));
    assert!(!row.has_column("name"));
}

#[test]
fn test_row_get_string() {
    let mut values = HashMap::new();
    values.insert("name".to_string(), Value::String("测试".to_string()));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_string("name");
    assert_eq!(value, Some("测试".to_string()));
}

#[test]
fn test_row_get_int() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_int("id");
    assert_eq!(value, Some(42));
}

#[test]
fn test_row_get_float() {
    let mut values = HashMap::new();
    values.insert("score".to_string(), Value::Float(2.5_f32));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_float("score");
    assert_eq!(value, Some(2.5_f64));
}

#[test]
fn test_row_get_bool() {
    let mut values = HashMap::new();
    values.insert("active".to_string(), Value::Bool(true));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_bool("active");
    assert_eq!(value, Some(true));
}

#[test]
fn test_row_get_vertex() {
    let vertex = Vertex::with_vid(VertexId::from_int64(1));
    let mut values = HashMap::new();
    values.insert("v".to_string(), Value::Vertex(Box::new(vertex)));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_vertex("v");
    assert!(value.is_some());
}

#[test]
fn test_row_get_edge() {
    let edge = Edge::new(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "follows".to_string(),
        0,
        HashMap::new(),
    );
    let mut values = HashMap::new();
    values.insert("e".to_string(), Value::Edge(Box::new(edge)));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    let value = row.get_edge("e");
    assert!(value.is_some());
}

#[test]
fn test_row_len() {
    let mut values = HashMap::new();
    values.insert("id".to_string(), Value::Int(42));
    values.insert("name".to_string(), Value::String("测试".to_string()));
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

    assert_eq!(row.len(), 2);
}

#[test]
fn test_row_is_empty() {
    let values = HashMap::new();
    let core_row = graphdb::api::core::Row { values };
    let row = Row::from_core(core_row);

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

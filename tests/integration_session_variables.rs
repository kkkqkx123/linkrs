//! Session variable (`$name`) integration tests through GraphService.
//!
//! Covers the `LET $name = expr` statement, parameter injection of session
//! variables into subsequent statements, and the transaction overlay
//! (ROLLBACK / ROLLBACK TO SAVEPOINT restore previous values).

use graphdb::api::server::graph_service::GraphService;
use graphdb::config::Config;
use graphdb::storage::{GraphStorage, SyncWrapper};
use graphdb::query::DataSet;
use graphdb::transaction::{TransactionManager, TransactionManagerConfig};
use std::sync::Arc;

async fn setup() -> (Arc<GraphService<SyncWrapper<GraphStorage>>>, i64) {
    let mut config = Config::default();
    config.server.auth.enable_authorize = false;
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let db_path = temp_dir.path().join("test.db");
    let storage = Arc::new(SyncWrapper::new(
        GraphStorage::new_with_path(db_path).expect("Failed to create storage"),
    ));
    let transaction_manager = Arc::new(TransactionManager::new(TransactionManagerConfig::default()));

    let graph_service =
        GraphService::new_with_transaction_manager(config, storage, transaction_manager).await;
    let session = graph_service
        .authenticate("root", "root")
        .await
        .expect("Root auth should succeed");
    let sid = session.id();

    graph_service
        .execute(sid, "CREATE SPACE sv_space (vid_type=INT64)")
        .await
        .expect("CREATE SPACE should succeed");
    graph_service
        .execute(sid, "USE sv_space")
        .await
        .expect("USE should succeed");
    graph_service
        .execute(sid, "CREATE TAG Person(name STRING, age INT)")
        .await
        .expect("CREATE TAG should succeed");

    (graph_service, sid)
}

async fn exec(
    service: &Arc<GraphService<SyncWrapper<GraphStorage>>>,
    sid: i64,
    stmt: &str,
) -> DataSet {
    match service.execute(sid, stmt).await {
        Ok(graphdb::query::executor::ExecutionResult::DataSet { data, .. }) => data,
        Ok(graphdb::query::executor::ExecutionResult::Success)
        | Ok(graphdb::query::executor::ExecutionResult::Empty) => {
            DataSet::from_rows(vec![], vec![])
        }
        other => panic!("statement `{}` failed: {:?}", stmt, other),
    }
}

async fn exec_err(service: &Arc<GraphService<SyncWrapper<GraphStorage>>>, sid: i64, stmt: &str) -> String {
    service
        .execute(sid, stmt)
        .await
        .expect_err(&format!("statement `{}` should fail", stmt))
}

fn first_scalar(data: &DataSet) -> graphdb::core::Value {
    data.rows
        .first()
        .expect("expected at least one row")
        .first()
        .expect("expected at least one column")
        .clone()
}

/// `LET $x = expr` stores a session variable; `$x` references resolve in
/// subsequent statements (including DML).
#[tokio::test]
async fn test_session_variable_let_and_reference() {
    let (service, sid) = setup().await;

    exec(&service, sid, "LET $x = 1 + 2").await;
    let data = exec(&service, sid, "RETURN $x").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Int(3),
        "$x should resolve to 1 + 2"
    );

    // String value.
    exec(&service, sid, "LET $name = 'Alice'").await;
    let data = exec(&service, sid, "RETURN $name").await;
    assert_eq!(first_scalar(&data), graphdb::core::Value::string("Alice"));

    // Variable references resolve inside DML.
    exec(
        &service,
        sid,
        "INSERT VERTEX Person(name, age) VALUES 1:($name, $x)",
    )
    .await;
    let data = exec(&service, sid, "FETCH PROP ON Person 1").await;
    let vertex = data
        .rows
        .first()
        .and_then(|row| row.first())
        .expect("FETCH should return the inserted vertex");
    match vertex {
        graphdb::core::Value::Vertex(v) => {
            let props = &v.tags[0].properties;
            assert_eq!(
                props.get("name"),
                Some(&graphdb::core::Value::string("Alice")),
                "inserted vertex should carry the $name value"
            );
            assert_eq!(
                props.get("age"),
                Some(&graphdb::core::Value::Int(3)),
                "inserted vertex should carry the $x value"
            );
        }
        other => panic!("expected a Vertex, got {:?}", other),
    }

    // Variable-to-variable assignment.
    exec(&service, sid, "LET $y = $x + 10").await;
    let data = exec(&service, sid, "RETURN $y").await;
    assert_eq!(first_scalar(&data), graphdb::core::Value::Int(13));
}

/// Assignments inside a transaction are rolled back with the transaction.
#[tokio::test]
async fn test_session_variable_transaction_rollback_restores() {
    let (service, sid) = setup().await;

    exec(&service, sid, "LET $x = 1").await;

    exec(&service, sid, "BEGIN").await;
    exec(&service, sid, "LET $x = 100").await;
    exec(&service, sid, "LET $y = 200").await;
    exec(&service, sid, "ROLLBACK").await;

    let data = exec(&service, sid, "RETURN $x").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Int(1),
        "ROLLBACK restores the pre-transaction variable"
    );
    let data = exec(&service, sid, "RETURN $y").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Null(Default::default()),
        "variable assigned inside a rolled-back transaction is undefined"
    );
}

/// COMMIT keeps assignments made inside the transaction.
#[tokio::test]
async fn test_session_variable_transaction_commit_merges() {
    let (service, sid) = setup().await;

    exec(&service, sid, "LET $x = 1").await;
    exec(&service, sid, "BEGIN").await;
    exec(&service, sid, "LET $x = 5").await;
    exec(&service, sid, "COMMIT").await;

    let data = exec(&service, sid, "RETURN $x").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Int(5),
        "COMMIT keeps the in-transaction assignment"
    );
}

/// ROLLBACK TO SAVEPOINT restores variables assigned after the savepoint.
#[tokio::test]
async fn test_session_variable_rollback_to_savepoint() {
    let (service, sid) = setup().await;

    exec(&service, sid, "LET $x = 1").await;

    exec(&service, sid, "BEGIN").await;
    exec(&service, sid, "LET $x = 2").await;
    exec(&service, sid, "SAVEPOINT sp1").await;
    exec(&service, sid, "LET $x = 3").await;
    exec(&service, sid, "LET $z = 4").await;
    exec(&service, sid, "ROLLBACK TO sp1").await;

    let data = exec(&service, sid, "RETURN $x").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Int(2),
        "ROLLBACK TO SAVEPOINT restores the value at the savepoint"
    );
    let data = exec(&service, sid, "RETURN $z").await;
    assert_eq!(
        first_scalar(&data),
        graphdb::core::Value::Null(Default::default()),
        "variable assigned after the savepoint is undefined"
    );

    exec(&service, sid, "COMMIT").await;
    let data = exec(&service, sid, "RETURN $x").await;
    assert_eq!(first_scalar(&data), graphdb::core::Value::Int(2));
}

/// Malformed LET statements fail with a clear error.
#[tokio::test]
async fn test_session_variable_let_errors() {
    let (service, sid) = setup().await;

    let err = exec_err(&service, sid, "LET $x").await;
    assert!(err.contains("LET requires an assignment"), "got: {}", err);

    let err = exec_err(&service, sid, "LET $ = 1").await;
    assert!(err.contains("Invalid session variable name"), "got: {}", err);

    let err = exec_err(&service, sid, "LET $1x = 1").await;
    assert!(err.contains("Invalid session variable name"), "got: {}", err);
}

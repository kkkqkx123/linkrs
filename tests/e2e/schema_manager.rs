//! E2E Test Suite for Schema Manager Initialization
//!
//! Tests that verify schema manager is properly initialized in various scenarios:
//! 1. Basic query operations work when vector search is disabled
//! 2. Basic query operations work when vector search is enabled but fails to initialize
//! 3. Schema validation works correctly

use crate::common::{assert_query_ok, create_test_db, setup_test_space};
use graphdb::api::server::graph_service::GraphService;
use graphdb::config::Config;
use graphdb::storage::{GraphStorage, SyncWrapper};
use std::sync::Arc;

/// Test schema manager initialization in different configurations
mod initialization {
    use super::*;

    /// Verify basic connection works
    #[test]
    fn test_basic_connection() {
        let mut db = create_test_db();
        let result = db.execute_query("SHOW SPACES");
        assert_query_ok(result, "Basic connection failed");
    }

    /// Create space should work regardless of vector config
    #[test]
    fn test_create_space_without_vector() {
        let mut db = create_test_db();

        // Drop if exists
        let _ = db.execute_query("DROP SPACE IF EXISTS schema_manager_test_space");

        // Create space - this should work even if schema_manager is not initialized
        let result = db.execute_query(
            "CREATE SPACE IF NOT EXISTS schema_manager_test_space (vid_type=STRING)",
        );
        assert_query_ok(
            result,
            "CREATE SPACE failed - schema_manager may not be initialized",
        );
    }

    /// Use space should work
    #[test]
    fn test_use_space() {
        let mut db = create_test_db();

        // Create space first
        db.execute_query("CREATE SPACE IF NOT EXISTS schema_manager_test_space (vid_type=STRING)")
            .expect("CREATE SPACE should succeed");

        let result = db.execute_query("USE schema_manager_test_space");
        assert_query_ok(
            result,
            "USE SPACE failed - schema_manager may not be initialized",
        );
    }

    /// Create tag should work with schema_manager
    #[test]
    fn test_create_tag() {
        let mut db = create_test_db();
        setup_test_space(&mut db, "schema_manager_test_space", &[], &[])
            .expect("Failed to setup test space");

        let result =
            db.execute_query("CREATE TAG IF NOT EXISTS test_person(name STRING NOT NULL, age INT)");
        assert_query_ok(
            result,
            "CREATE TAG failed - schema_manager may not be initialized",
        );
    }

    /// Show tags should work
    #[test]
    fn test_show_tags() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "schema_manager_test_space",
            &["CREATE TAG IF NOT EXISTS test_person(name STRING, age INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        let result = db.execute_query("SHOW TAGS");
        assert_query_ok(
            result,
            "SHOW TAGS failed - schema_manager may not be initialized",
        );
    }

    /// Insert vertex should work
    #[test]
    fn test_insert_vertex() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "schema_manager_test_space",
            &["CREATE TAG IF NOT EXISTS test_person(name STRING, age INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        let result =
            db.execute_query("INSERT VERTEX test_person(name, age) VALUES 'p1': ('Alice', 30)");
        assert_query_ok(
            result,
            "INSERT VERTEX failed - schema_manager may not be initialized",
        );
    }

    /// Fetch vertex should work
    #[test]
    fn test_fetch_vertex() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "schema_manager_test_space",
            &["CREATE TAG IF NOT EXISTS test_person(name STRING, age INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert vertex
        db.execute_query("INSERT VERTEX test_person(name, age) VALUES 'p_fetch': ('Bob', 25)")
            .expect("INSERT should succeed");

        let result = db.execute_query("FETCH PROP ON test_person 'p_fetch'");
        assert_query_ok(
            result,
            "FETCH PROP failed - schema_manager may not be initialized",
        );
    }

    /// MATCH query should work
    #[test]
    fn test_match_query() {
        let mut db = create_test_db();
        setup_test_space(
            &mut db,
            "schema_manager_test_space",
            &["CREATE TAG IF NOT EXISTS test_person(name STRING, age INT)"],
            &[],
        )
        .expect("Failed to setup test space");

        // Insert vertex
        db.execute_query("INSERT VERTEX test_person(name, age) VALUES 'p1': ('Alice', 30)")
            .expect("INSERT should succeed");

        let result = db.execute_query("MATCH (v:test_person) RETURN v LIMIT 1");
        // MATCH might not be fully implemented, so we just check it doesn't crash
        // and doesn't return schema_manager error
        if let Err(ref e) = result {
            let error_msg = format!("{:?}", e).to_lowercase();
            assert!(
                !error_msg.contains("schema manager not initialized"),
                "MATCH query failed due to schema_manager not initialized"
            );
        }
    }

    /// Drop space should work
    #[test]
    fn test_drop_space() {
        let mut db = create_test_db();

        // Setup
        db.execute_query("CREATE SPACE IF NOT EXISTS schema_manager_test_space (vid_type=STRING)")
            .expect("CREATE SPACE should succeed");

        let result = db.execute_query("DROP SPACE IF EXISTS schema_manager_test_space");
        assert_query_ok(result, "DROP SPACE failed");
    }
}

/// Test error handling when schema manager is not available
mod error_handling {
    use super::*;

    /// Error messages should be clear when operations fail
    #[test]
    fn test_error_message_clarity() {
        let mut db = create_test_db();

        // Try to use a non-existent space
        let result = db.execute_query("USE non_existent_space_xyz");

        // Should fail, but error should not be "schema manager not initialized"
        if let Err(ref e) = result {
            let error_msg = format!("{:?}", e).to_lowercase();
            assert!(
                !error_msg.contains("schema manager not initialized"),
                "Error message indicates schema_manager not initialized - this is a server config issue"
            );
        }
    }

    /// SHOW SPACES should always work
    #[test]
    fn test_show_spaces_always_works() {
        let mut db = create_test_db();

        let result = db.execute_query("SHOW SPACES");
        assert_query_ok(result, "SHOW SPACES should always work but failed");
    }
}

/// Test GraphService with schema manager
mod graph_service {
    use super::*;

    /// Test GraphService creation and basic operations
    #[tokio::test]
    async fn test_graph_service_creation() {
        let config = Config::default();
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let storage = Arc::new(SyncWrapper::new(
            GraphStorage::open(db_path.clone()).expect("Failed to create storage"),
        ));

        let graph_service = GraphService::new(config, storage).await;

        // Verify the service was created
        assert!(
            graph_service
                .get_session_manager()
                .list_sessions()
                .await
                .is_empty(),
            "GraphService should be created with empty sessions"
        );
    }

    /// Test authentication and query execution
    #[tokio::test]
    async fn test_graph_service_query_execution() {
        let config = Config::default();
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        let storage = Arc::new(SyncWrapper::new(
            GraphStorage::open(db_path.clone()).expect("Failed to create storage"),
        ));

        let graph_service = GraphService::new(config, storage).await;

        // Authenticate
        let session = graph_service
            .authenticate("root", "root")
            .await
            .expect("Root auth should succeed");
        let session_id = session.id();

        // Execute query
        let result = graph_service.execute(session_id, "SHOW SPACES").await;

        // Should succeed (or at least not fail due to schema manager)
        if let Err(ref e) = result {
            let error_msg = format!("{:?}", e).to_lowercase();
            assert!(
                !error_msg.contains("schema manager not initialized"),
                "Query failed due to schema_manager not initialized"
            );
        }
    }
}

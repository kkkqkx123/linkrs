//! C API Integration Testing Assistant Tool
//!
//! Provide public functions and structures for testing the C API.

#![allow(dead_code)]

use std::ffi::CString;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::TempDir;

use graphdb::api::embedded::c_api::error::graphdb_error_code_t;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// C API for testing database wrappers
///
/// Use the RAII (Resource Acquisition Is Initialization) pattern to manage the database lifecycle, ensuring that resources are properly cleaned up after testing.
pub struct CApiTestDatabase {
    db: *mut graphdb::api::embedded::c_api::types::graphdb_t,
    temp_dir: TempDir,
}

impl CApiTestDatabase {
    /// Create a new test database.
    ///
    /// Use a temporary directory to create independent database files to ensure the isolation of the tests.
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("创建临时目录失败");
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let db_path = temp_dir.path().join(format!("test_{}.db", counter));

        let path_cstring =
            CString::new(db_path.to_str().expect("路径转换为字符串失败")).expect("创建CString失败");
        let mut db: *mut graphdb::api::embedded::c_api::types::graphdb_t = ptr::null_mut();

        let rc = unsafe {
            graphdb::api::embedded::c_api::database::graphdb_open(path_cstring.as_ptr(), &mut db)
        };

        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "打开数据库失败，错误码: {}, 路径: {:?}",
            rc,
            db_path
        );
        assert!(!db.is_null(), "The database handle should not be empty.");

        Self { db, temp_dir }
    }

    /// Obtaining a database handle
    pub fn handle(&self) -> *mut graphdb::api::embedded::c_api::types::graphdb_t {
        self.db
    }
}

impl Drop for CApiTestDatabase {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                graphdb::api::embedded::c_api::database::graphdb_close(self.db);
            }
        }
    }
}

/// C API Test Session Wrapper
///
/// Managing the session lifecycle using the RAII (Resource Acquisition Is Initialization) pattern
pub struct CApiTestSession {
    session: *mut graphdb::api::embedded::c_api::types::graphdb_session_t,
}

impl CApiTestSession {
    /// Create a session from the database.
    pub fn from_db(db: &CApiTestDatabase) -> Self {
        let mut session: *mut graphdb::api::embedded::c_api::types::graphdb_session_t =
            ptr::null_mut();

        let rc = unsafe {
            graphdb::api::embedded::c_api::session::graphdb_session_create(
                db.handle(),
                &mut session,
            )
        };

        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "Failed to create a session."
        );
        assert!(
            !session.is_null(),
            "The session handle should not be empty."
        );

        Self { session }
    }

    /// Obtaining the session handle
    pub fn handle(&self) -> *mut graphdb::api::embedded::c_api::types::graphdb_session_t {
        self.session
    }
}

impl Drop for CApiTestSession {
    fn drop(&mut self) {
        if !self.session.is_null() {
            unsafe {
                graphdb::api::embedded::c_api::session::graphdb_session_close(self.session);
            }
        }
    }
}

/// C API Test Transaction Wrapper
///
/// Managing the transaction lifecycle using the RAII pattern
pub struct CApiTestTransaction {
    txn: *mut graphdb::api::embedded::c_api::types::graphdb_txn_t,
}

impl CApiTestTransaction {
    /// Create a transaction from the session
    pub fn from_session(session: &CApiTestSession) -> Self {
        let mut txn: *mut graphdb::api::embedded::c_api::types::graphdb_txn_t = ptr::null_mut();

        let rc = unsafe {
            graphdb::api::embedded::c_api::transaction::graphdb_txn_begin(
                session.handle(),
                &mut txn,
            )
        };

        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "Failed to start the transaction."
        );
        assert!(
            !txn.is_null(),
            "The transaction handle should not be empty."
        );

        Self { txn }
    }

    /// Obtaining the transaction handle
    pub fn handle(&self) -> *mut graphdb::api::embedded::c_api::types::graphdb_txn_t {
        self.txn
    }

    /// Commit a transaction
    pub fn commit(self) {
        let rc =
            unsafe { graphdb::api::embedded::c_api::transaction::graphdb_txn_commit(self.txn) };
        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "The transaction failed to be committed."
        );
        // Prevent the component from being released again when the “Drop” event occurs.
        std::mem::forget(self);
    }

    /// Roll back a transaction
    pub fn rollback(self) {
        let rc =
            unsafe { graphdb::api::embedded::c_api::transaction::graphdb_txn_rollback(self.txn) };
        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "Rolling back the transaction failed."
        );
        // Prevent the object from being released again when the “Drop” event occurs.
        std::mem::forget(self);
    }
}

impl Drop for CApiTestTransaction {
    fn drop(&mut self) {
        if !self.txn.is_null() {
            unsafe {
                graphdb::api::embedded::c_api::transaction::graphdb_txn_free(self.txn);
            }
        }
    }
}

/// C API Test Results Wrapper
///
/// Manage the lifecycle of result sets using the RAII (Resource Acquisition Is Initialization) pattern.
pub struct CApiTestResult {
    result: *mut graphdb::api::embedded::c_api::types::graphdb_result_t,
}

impl CApiTestResult {
    /// Create results from executing queries within the conversation.
    pub fn from_query(session: &CApiTestSession, query: &str) -> Self {
        let query_cstring = CString::new(query).expect("查询字符串无效");
        let mut result: *mut graphdb::api::embedded::c_api::types::graphdb_result_t =
            ptr::null_mut();

        let rc = unsafe {
            graphdb::api::embedded::c_api::query::graphdb_execute(
                session.handle(),
                query_cstring.as_ptr(),
                &mut result,
            )
        };

        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "The query failed to be executed."
        );
        assert!(!result.is_null(), "The result handle should not be empty.");

        Self { result }
    }

    /// Get the number of columns
    pub fn column_count(&self) -> i32 {
        unsafe { graphdb::api::embedded::c_api::result::graphdb_column_count(self.result) }
    }

    /// Get the number of rows
    pub fn row_count(&self) -> i32 {
        unsafe { graphdb::api::embedded::c_api::result::graphdb_row_count(self.result) }
    }
}

impl Drop for CApiTestResult {
    fn drop(&mut self) {
        if !self.result.is_null() {
            unsafe {
                graphdb::api::embedded::c_api::result::graphdb_result_free(self.result);
            }
        }
    }
}

/// C API Test for Batch Operation Wrapper
///
/// Using the RAII (Resource Acquisition Is Initialization) pattern to manage the lifecycle of batch operations
pub struct CApiTestBatch {
    batch: *mut graphdb::api::embedded::c_api::types::graphdb_batch_t,
}

impl CApiTestBatch {
    /// Create a batch inserter from the session
    pub fn from_session(session: &CApiTestSession, batch_size: i32) -> Self {
        let mut batch: *mut graphdb::api::embedded::c_api::types::graphdb_batch_t = ptr::null_mut();

        let rc = unsafe {
            graphdb::api::embedded::c_api::batch::graphdb_batch_inserter_create(
                session.handle(),
                batch_size,
                &mut batch,
            )
        };

        assert_eq!(
            rc,
            graphdb_error_code_t::GRAPHDB_OK as i32,
            "Failed to create the batch inserter."
        );
        assert!(
            !batch.is_null(),
            "The batch operation handle should not be empty."
        );

        Self { batch }
    }

    /// Obtain batch operation handles
    pub fn handle(&self) -> *mut graphdb::api::embedded::c_api::types::graphdb_batch_t {
        self.batch
    }
}

impl Drop for CApiTestBatch {
    fn drop(&mut self) {
        if !self.batch.is_null() {
            unsafe {
                graphdb::api::embedded::c_api::batch::graphdb_batch_free(self.batch);
            }
        }
    }
}

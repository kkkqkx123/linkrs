//! C API Integration Testing
//!
//! Test scope:
//! Database Lifecycle Management
//! Session management
//! Query execution
//! Result processing
//! Transaction Management
//! Precompiled statements
//! Batch operations
//! Error handling

#![cfg(feature = "embedded")]

mod common;

use std::ffi::CString;
use std::ptr;

use graphdb::api::embedded::c_api::error::graphdb_error_code_t;

use common::c_api_helpers::{
    CApiTestBatch, CApiTestDatabase, CApiTestResult, CApiTestSession, CApiTestTransaction,
};

// ==================== Database Lifecycle Testing ====================

#[test]
fn test_c_api_database_open_close() {
    let test_db = CApiTestDatabase::new();
    let db = test_db.handle();

    assert!(!db.is_null());

    // The database will be automatically closed when the “Drop” command is executed.
}

#[test]
fn test_c_api_libversion() {
    let version = unsafe {
        std::ffi::CStr::from_ptr(graphdb::api::embedded::c_api::database::graphdb_libversion())
    };

    let version_str = version.to_str().expect("版本字符串无效");
    assert!(!version_str.is_empty());
}

#[test]
fn test_c_api_database_null_params() {
    let rc = unsafe {
        graphdb::api::embedded::c_api::database::graphdb_open(ptr::null(), ptr::null_mut())
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as i32);
}

#[test]
fn test_c_api_database_multiple_open_close() {
    let test_db1 = CApiTestDatabase::new();
    let test_db2 = CApiTestDatabase::new();

    assert!(!test_db1.handle().is_null());
    assert!(!test_db2.handle().is_null());

    // Verify that the two database handles are different.
    assert_ne!(test_db1.handle(), test_db2.handle());
}

// ==================== Session Management Test ====================

#[test]
fn test_c_api_session_create_close() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    assert!(!session.handle().is_null());
}

#[test]
fn test_c_api_session_autocommit() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    // Default automatic submission
    let autocommit = unsafe {
        graphdb::api::embedded::c_api::session::graphdb_session_get_autocommit(session.handle())
    };
    assert!(autocommit);

    // Turn off automatic submission.
    let rc = unsafe {
        graphdb::api::embedded::c_api::session::graphdb_session_set_autocommit(
            session.handle(),
            false,
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);

    let autocommit = unsafe {
        graphdb::api::embedded::c_api::session::graphdb_session_get_autocommit(session.handle())
    };
    assert!(!autocommit);
}

#[test]
fn test_c_api_session_null_params() {
    let rc = unsafe {
        graphdb::api::embedded::c_api::session::graphdb_session_create(
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as i32);
}

#[test]
fn test_c_api_session_multiple_sessions() {
    let test_db = CApiTestDatabase::new();
    let session1 = CApiTestSession::from_db(&test_db);
    let session2 = CApiTestSession::from_db(&test_db);

    // Verify that the two session handles are different.
    assert_ne!(session1.handle(), session2.handle());
}

// ==================== Query Execution Test ====================

#[test]
fn test_c_api_execute_simple_query() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let query = CString::new("SHOW SPACES").expect("创建CString失败");
    let mut result: *mut graphdb::api::embedded::c_api::types::graphdb_result_t = ptr::null_mut();

    let rc = unsafe {
        graphdb::api::embedded::c_api::query::graphdb_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };

    // Printing error messages is used for debugging purposes.
    if rc != graphdb_error_code_t::GRAPHDB_OK as i32 {
        let error_msg = graphdb::api::embedded::c_api::error::graphdb_get_last_error_message();
        if !error_msg.is_null() {
            let _msg = unsafe {
                std::ffi::CStr::from_ptr(error_msg)
                    .to_string_lossy()
                    .to_string()
            };
        }
    }

    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);
    assert!(!result.is_null());

    // Cleanup results
    unsafe {
        graphdb::api::embedded::c_api::result::graphdb_result_free(result);
    }
}

#[test]
fn test_c_api_execute_with_wrapper() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let result = CApiTestResult::from_query(&session, "SHOW SPACES");

    assert!(result.column_count() >= 0);
    assert!(result.row_count() >= 0);
}

#[test]
fn test_c_api_execute_null_params() {
    let rc = unsafe {
        graphdb::api::embedded::c_api::query::graphdb_execute(
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as i32);
}

// ==================== Result Processing Test ====================

#[test]
fn test_c_api_result_metadata() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let query = CString::new("SHOW SPACES").expect("创建CString失败");
    let mut result: *mut graphdb::api::embedded::c_api::types::graphdb_result_t = ptr::null_mut();

    let rc = unsafe {
        graphdb::api::embedded::c_api::query::graphdb_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);

    // Get the number of columns
    let col_count = unsafe { graphdb::api::embedded::c_api::result::graphdb_column_count(result) };
    assert!(col_count >= 0);

    // Get the number of rows
    let row_count = unsafe { graphdb::api::embedded::c_api::result::graphdb_row_count(result) };
    assert!(row_count >= 0);

    // Cleanup results
    unsafe {
        graphdb::api::embedded::c_api::result::graphdb_result_free(result);
    }
}

#[test]
fn test_c_api_result_column_name() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let query = CString::new("SHOW SPACES").expect("创建CString失败");
    let mut result: *mut graphdb::api::embedded::c_api::types::graphdb_result_t = ptr::null_mut();

    let rc = unsafe {
        graphdb::api::embedded::c_api::query::graphdb_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);

    // Get the number of columns
    let col_count = unsafe { graphdb::api::embedded::c_api::result::graphdb_column_count(result) };

    if col_count > 0 {
        // Get the name of the first column.
        let col_name =
            unsafe { graphdb::api::embedded::c_api::result::graphdb_column_name(result, 0) };

        if !col_name.is_null() {
            let name = unsafe { std::ffi::CStr::from_ptr(col_name) };
            let name_str = name.to_str().expect("列名无效");
            assert!(!name_str.is_empty());

            // Release the column name string.
            unsafe {
                graphdb::api::embedded::c_api::database::graphdb_free_string(col_name);
            }
        }
    }

    // Clean-up results
    unsafe {
        graphdb::api::embedded::c_api::result::graphdb_result_free(result);
    }
}

#[test]
fn test_c_api_result_null_params() {
    let count =
        unsafe { graphdb::api::embedded::c_api::result::graphdb_column_count(ptr::null_mut()) };
    assert_eq!(count, -1);

    let count =
        unsafe { graphdb::api::embedded::c_api::result::graphdb_row_count(ptr::null_mut()) };
    assert_eq!(count, -1);

    let name =
        unsafe { graphdb::api::embedded::c_api::result::graphdb_column_name(ptr::null_mut(), 0) };
    assert!(name.is_null());
}

// ==================== Transaction Management Testing ====================

#[test]
fn test_c_api_transaction_begin_commit() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut txn: *mut graphdb::api::embedded::c_api::types::graphdb_txn_t = ptr::null_mut();

    // Start a transaction
    let rc = unsafe {
        graphdb::api::embedded::c_api::transaction::graphdb_txn_begin(session.handle(), &mut txn)
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);
    assert!(!txn.is_null());

    // Submit the transaction
    let rc = unsafe { graphdb::api::embedded::c_api::transaction::graphdb_txn_commit(txn) };

    // Printing error messages is used for debugging.
    if rc != graphdb_error_code_t::GRAPHDB_OK as i32 {
        let error_msg = graphdb::api::embedded::c_api::error::graphdb_get_last_error_message();
        if !error_msg.is_null() {
            let _msg = unsafe {
                std::ffi::CStr::from_ptr(error_msg)
                    .to_string_lossy()
                    .to_string()
            };
        }
    }

    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);

    // Clean up transaction handlers
    unsafe {
        graphdb::api::embedded::c_api::transaction::graphdb_txn_free(txn);
    }
}

#[test]
fn test_c_api_transaction_begin_rollback() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut txn: *mut graphdb::api::embedded::c_api::types::graphdb_txn_t = ptr::null_mut();

    // Start a transaction
    let rc = unsafe {
        graphdb::api::embedded::c_api::transaction::graphdb_txn_begin(session.handle(), &mut txn)
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);
    assert!(!txn.is_null());

    // Roll back a transaction
    let rc = unsafe { graphdb::api::embedded::c_api::transaction::graphdb_txn_rollback(txn) };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);

    // Clean up transaction handlers
    unsafe {
        graphdb::api::embedded::c_api::transaction::graphdb_txn_free(txn);
    }
}

#[test]
fn test_c_api_transaction_with_wrapper() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let txn = CApiTestTransaction::from_session(&session);
    assert!(!txn.handle().is_null());

    // Submit the transaction
    txn.commit();
}

#[test]
fn test_c_api_transaction_rollback_with_wrapper() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let txn = CApiTestTransaction::from_session(&session);
    assert!(!txn.handle().is_null());

    // Roll back a transaction
    txn.rollback();
}

#[test]
fn test_c_api_transaction_null_params() {
    let rc = unsafe {
        graphdb::api::embedded::c_api::transaction::graphdb_txn_begin(
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as i32);
}

// ==================== Batch Operation Testing ====================

#[test]
fn test_c_api_batch_inserter_create_free() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut batch: *mut graphdb::api::embedded::c_api::types::graphdb_batch_t = ptr::null_mut();

    // Create a batch inserter
    let rc = unsafe {
        graphdb::api::embedded::c_api::batch::graphdb_batch_inserter_create(
            session.handle(),
            100,
            &mut batch,
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);
    assert!(!batch.is_null());

    // Release the batch inserter.
    let rc = unsafe { graphdb::api::embedded::c_api::batch::graphdb_batch_free(batch) };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_OK as i32);
}

#[test]
fn test_c_api_batch_with_wrapper() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let batch = CApiTestBatch::from_session(&session, 100);
    assert!(!batch.handle().is_null());
}

#[test]
fn test_c_api_batch_null_params() {
    let rc = unsafe {
        graphdb::api::embedded::c_api::batch::graphdb_batch_inserter_create(
            ptr::null_mut(),
            100,
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, graphdb_error_code_t::GRAPHDB_MISUSE as i32);
}

#[test]
fn test_c_api_batch_buffered_counts_null() {
    let count = unsafe {
        graphdb::api::embedded::c_api::batch::graphdb_batch_buffered_vertices(ptr::null_mut())
    };
    assert_eq!(count, -1);

    let count = unsafe {
        graphdb::api::embedded::c_api::batch::graphdb_batch_buffered_edges(ptr::null_mut())
    };
    assert_eq!(count, -1);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_c_api_error_string() {
    let error_str = unsafe {
        std::ffi::CStr::from_ptr(graphdb::api::embedded::c_api::error::graphdb_error_string(
            graphdb_error_code_t::GRAPHDB_OK as i32,
        ))
    };

    let desc = error_str.to_str().expect("Invalid error description");
    assert_eq!(desc, "OK");
}

#[test]
fn test_c_api_error_codes() {
    let test_cases = vec![
        (graphdb_error_code_t::GRAPHDB_OK as i32, "OK"),
        (graphdb_error_code_t::GRAPHDB_ERROR as i32, "General error"),
        (graphdb_error_code_t::GRAPHDB_MISUSE as i32, "Misuse"),
        (graphdb_error_code_t::GRAPHDB_NOTFOUND as i32, "Not found"),
        (graphdb_error_code_t::GRAPHDB_IOERR as i32, "IO error"),
        (
            graphdb_error_code_t::GRAPHDB_CORRUPT as i32,
            "Data corruption",
        ),
        (graphdb_error_code_t::GRAPHDB_NOMEM as i32, "Out of memory"),
    ];

    for (code, expected_desc) in test_cases {
        let error_str = unsafe {
            std::ffi::CStr::from_ptr(graphdb::api::embedded::c_api::error::graphdb_error_string(
                code,
            ))
        };

        let desc = error_str.to_str().expect("Invalid error description");
        assert_eq!(
            desc, expected_desc,
            "Description mismatch for error code {}",
            code
        );
    }
}

#[test]
fn test_c_api_errmsg() {
    let mut buffer = [0i8; 256];
    let len = unsafe {
        graphdb::api::embedded::c_api::error::graphdb_errmsg(buffer.as_mut_ptr(), buffer.len())
    };

    // Verify that the returned length is reasonable.
    assert!(len >= 0);
    assert!((len as usize) < buffer.len());
}

// ==================== Memory Management Tests ====================

#[test]
fn test_c_api_free_string() {
    let test_str = CString::new("test string").expect("创建CString失败");
    let ptr = test_str.into_raw();

    assert!(!ptr.is_null());

    // Release the string
    unsafe {
        graphdb::api::embedded::c_api::database::graphdb_free_string(ptr);
    }
}

#[test]
fn test_c_api_free() {
    let test_value = Box::new(42i32);
    let ptr = Box::into_raw(test_value) as *mut std::ffi::c_void;

    assert!(!ptr.is_null());

    // Free up memory
    unsafe {
        graphdb::api::embedded::c_api::database::graphdb_free(ptr);
    }
}

// ==================== Integrated Scenario Testing ====================

#[test]
fn test_c_api_full_workflow() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    // Please provide the text you would like to have translated.
    let result = CApiTestResult::from_query(&session, "SHOW SPACES");

    // Verification results
    assert!(result.column_count() >= 0);
    assert!(result.row_count() >= 0);

    // Commencement of business
    let txn = CApiTestTransaction::from_session(&session);

    // Submission of transactions
    txn.commit();
    // All resources are automatically cleaned up at Drop
}

#[test]
fn test_c_api_concurrent_sessions() {
    let test_db = CApiTestDatabase::new();

    let session1 = CApiTestSession::from_db(&test_db);
    let session2 = CApiTestSession::from_db(&test_db);
    let session3 = CApiTestSession::from_db(&test_db);

    // Verify that all three sessions are valid
    assert!(!session1.handle().is_null());
    assert!(!session2.handle().is_null());
    assert!(!session3.handle().is_null());

    // Verify that the session handles are different
    assert_ne!(session1.handle(), session2.handle());
    assert_ne!(session2.handle(), session3.handle());
    assert_ne!(session1.handle(), session3.handle());
}

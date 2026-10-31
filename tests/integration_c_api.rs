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

use std::ffi::CString;
use std::ptr;

use linkrs::api::embedded::c_api::error::linkrs_error_code_t;

use linkrs::test_utils::c_api_helpers::{
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
        std::ffi::CStr::from_ptr(linkrs::api::embedded::c_api::database::linkrs_libversion())
    };

    let version_str = version.to_str().expect("版本字符串无效");
    assert!(!version_str.is_empty());
}

#[test]
fn test_c_api_database_null_params() {
    let rc = unsafe {
        linkrs::api::embedded::c_api::database::linkrs_open(ptr::null(), ptr::null_mut())
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as i32);
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
        linkrs::api::embedded::c_api::session::linkrs_session_get_autocommit(session.handle())
    };
    assert!(autocommit);

    // Turn off automatic submission.
    let rc = unsafe {
        linkrs::api::embedded::c_api::session::linkrs_session_set_autocommit(
            session.handle(),
            false,
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    let autocommit = unsafe {
        linkrs::api::embedded::c_api::session::linkrs_session_get_autocommit(session.handle())
    };
    assert!(!autocommit);
}

#[test]
fn test_c_api_session_null_params() {
    let rc = unsafe {
        linkrs::api::embedded::c_api::session::linkrs_session_create(
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as i32);
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
    let mut result: *mut linkrs::api::embedded::c_api::types::linkrs_result_t = ptr::null_mut();

    let rc = unsafe {
        linkrs::api::embedded::c_api::query::linkrs_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };

    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
    assert!(!result.is_null());

    // Cleanup results
    unsafe {
        linkrs::api::embedded::c_api::result::linkrs_result_free(result);
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
        linkrs::api::embedded::c_api::query::linkrs_execute(
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as i32);
}

// ==================== Result Processing Test ====================

#[test]
fn test_c_api_result_metadata() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let query = CString::new("SHOW SPACES").expect("创建CString失败");
    let mut result: *mut linkrs::api::embedded::c_api::types::linkrs_result_t = ptr::null_mut();

    let rc = unsafe {
        linkrs::api::embedded::c_api::query::linkrs_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    // Get the number of columns
    let col_count = unsafe { linkrs::api::embedded::c_api::result::linkrs_column_count(result) };
    assert!(col_count >= 0);

    // Get the number of rows
    let row_count = unsafe { linkrs::api::embedded::c_api::result::linkrs_row_count(result) };
    assert!(row_count >= 0);

    // Cleanup results
    unsafe {
        linkrs::api::embedded::c_api::result::linkrs_result_free(result);
    }
}

#[test]
fn test_c_api_result_column_name() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let query = CString::new("SHOW SPACES").expect("创建CString失败");
    let mut result: *mut linkrs::api::embedded::c_api::types::linkrs_result_t = ptr::null_mut();

    let rc = unsafe {
        linkrs::api::embedded::c_api::query::linkrs_execute(
            session.handle(),
            query.as_ptr(),
            &mut result,
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    // Get the number of columns
    let col_count = unsafe { linkrs::api::embedded::c_api::result::linkrs_column_count(result) };

    if col_count > 0 {
        // Get the name of the first column.
        let col_name =
            unsafe { linkrs::api::embedded::c_api::result::linkrs_column_name(result, 0) };

        if !col_name.is_null() {
            let name = unsafe { std::ffi::CStr::from_ptr(col_name) };
            let name_str = name.to_str().expect("列名无效");
            assert!(!name_str.is_empty());

            // Release the column name string.
            unsafe {
                linkrs::api::embedded::c_api::database::linkrs_free_string(col_name);
            }
        }
    }

    // Clean-up results
    unsafe {
        linkrs::api::embedded::c_api::result::linkrs_result_free(result);
    }
}

#[test]
fn test_c_api_result_null_params() {
    let count =
        unsafe { linkrs::api::embedded::c_api::result::linkrs_column_count(ptr::null_mut()) };
    assert_eq!(count, -1);

    let count = unsafe { linkrs::api::embedded::c_api::result::linkrs_row_count(ptr::null_mut()) };
    assert_eq!(count, -1);

    let name =
        unsafe { linkrs::api::embedded::c_api::result::linkrs_column_name(ptr::null_mut(), 0) };
    assert!(name.is_null());
}

// ==================== Transaction Management Testing ====================

#[test]
fn test_c_api_transaction_begin_commit() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut txn: *mut linkrs::api::embedded::c_api::types::linkrs_txn_t = ptr::null_mut();

    // Start a transaction
    let rc = unsafe {
        linkrs::api::embedded::c_api::transaction::linkrs_txn_begin(session.handle(), &mut txn)
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
    assert!(!txn.is_null());

    // Submit the transaction
    let rc = unsafe { linkrs::api::embedded::c_api::transaction::linkrs_txn_commit(txn) };

    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    // Clean up transaction handlers
    unsafe {
        linkrs::api::embedded::c_api::transaction::linkrs_txn_free(txn);
    }
}

#[test]
fn test_c_api_transaction_begin_rollback() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut txn: *mut linkrs::api::embedded::c_api::types::linkrs_txn_t = ptr::null_mut();

    // Start a transaction
    let rc = unsafe {
        linkrs::api::embedded::c_api::transaction::linkrs_txn_begin(session.handle(), &mut txn)
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
    assert!(!txn.is_null());

    // Roll back a transaction
    let rc = unsafe { linkrs::api::embedded::c_api::transaction::linkrs_txn_rollback(txn) };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    // Clean up transaction handlers
    unsafe {
        linkrs::api::embedded::c_api::transaction::linkrs_txn_free(txn);
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
        linkrs::api::embedded::c_api::transaction::linkrs_txn_begin(
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as i32);
}

// ==================== Batch Operation Testing ====================

#[test]
fn test_c_api_batch_inserter_create_free() {
    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);

    let mut batch: *mut linkrs::api::embedded::c_api::types::linkrs_batch_t = ptr::null_mut();

    // Create a batch inserter
    let rc = unsafe {
        linkrs::api::embedded::c_api::batch::linkrs_batch_inserter_create(
            session.handle(),
            100,
            &mut batch,
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
    assert!(!batch.is_null());

    // Release the batch inserter.
    let rc = unsafe { linkrs::api::embedded::c_api::batch::linkrs_batch_free(batch) };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
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
        linkrs::api::embedded::c_api::batch::linkrs_batch_inserter_create(
            ptr::null_mut(),
            100,
            ptr::null_mut(),
        )
    };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_MISUSE as i32);
}

#[test]
fn test_c_api_batch_buffered_counts_null() {
    let count = unsafe {
        linkrs::api::embedded::c_api::batch::linkrs_batch_buffered_vertices(ptr::null_mut())
    };
    assert_eq!(count, -1);

    let count = unsafe {
        linkrs::api::embedded::c_api::batch::linkrs_batch_buffered_edges(ptr::null_mut())
    };
    assert_eq!(count, -1);
}

// ==================== Error Handling Tests ====================

#[test]
fn test_c_api_errstr() {
    let error_str = unsafe {
        std::ffi::CStr::from_ptr(linkrs::api::embedded::c_api::error::linkrs_errstr(
            linkrs_error_code_t::LINKRS_OK as i32,
        ))
    };

    let desc = error_str.to_str().expect("Invalid error description");
    assert_eq!(desc, "OK");
}

#[test]
fn test_c_api_error_codes() {
    let test_cases = vec![
        (linkrs_error_code_t::LINKRS_OK as i32, "OK"),
        (linkrs_error_code_t::LINKRS_ERROR as i32, "General error"),
        (linkrs_error_code_t::LINKRS_MISUSE as i32, "Misuse"),
        (linkrs_error_code_t::LINKRS_NOTFOUND as i32, "Not found"),
        (linkrs_error_code_t::LINKRS_IOERR as i32, "IO error"),
        (
            linkrs_error_code_t::LINKRS_CORRUPT as i32,
            "Data corruption",
        ),
        (linkrs_error_code_t::LINKRS_NOMEM as i32, "Out of memory"),
    ];

    for (code, expected_desc) in test_cases {
        let error_str = unsafe {
            std::ffi::CStr::from_ptr(linkrs::api::embedded::c_api::error::linkrs_errstr(code))
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
        linkrs::api::embedded::c_api::error::linkrs_errmsg(buffer.as_mut_ptr(), buffer.len())
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
        linkrs::api::embedded::c_api::database::linkrs_free_string(ptr);
    }
}

#[test]
fn test_c_api_free() {
    let test_value = Box::new(42i32);
    let ptr = Box::into_raw(test_value) as *mut std::ffi::c_void;

    assert!(!ptr.is_null());

    // Free up memory
    unsafe {
        linkrs::api::embedded::c_api::database::linkrs_free(ptr);
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

// ==================== Session Variables ====================

unsafe fn take_c_string(ptr: *mut std::ffi::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = std::ffi::CStr::from_ptr(ptr)
        .to_str()
        .ok()
        .map(|value| value.to_string());
    linkrs::api::embedded::c_api::database::linkrs_free_string(ptr);
    text
}

#[test]
fn test_c_api_session_variables_roundtrip() {
    use linkrs::api::embedded::c_api::session::{
        linkrs_session_get_variable, linkrs_session_list_variables, linkrs_session_remove_variable,
        linkrs_session_set_variable,
    };

    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);
    let name = CString::new("limit").expect("valid name");
    let value = CString::new("42").expect("valid json");

    let rc =
        unsafe { linkrs_session_set_variable(session.handle(), name.as_ptr(), value.as_ptr()) };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);

    let stored =
        unsafe { take_c_string(linkrs_session_get_variable(session.handle(), name.as_ptr())) };
    assert_eq!(stored.as_deref(), Some("42"));

    let listed = unsafe { take_c_string(linkrs_session_list_variables(session.handle())) }
        .expect("non-null variable list");
    let parsed: serde_json::Value = serde_json::from_str(&listed).expect("valid JSON object");
    assert_eq!(parsed.get("limit"), Some(&serde_json::json!(42)));

    let rc = unsafe { linkrs_session_remove_variable(session.handle(), name.as_ptr()) };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_OK as i32);
    let rc = unsafe { linkrs_session_remove_variable(session.handle(), name.as_ptr()) };
    assert_eq!(rc, linkrs_error_code_t::LINKRS_NOTFOUND as i32);
    let stored =
        unsafe { take_c_string(linkrs_session_get_variable(session.handle(), name.as_ptr())) };
    assert!(stored.is_none());
}

#[test]
fn test_c_api_session_variables_invalid_arguments() {
    use linkrs::api::embedded::c_api::session::{
        linkrs_session_get_variable, linkrs_session_list_variables, linkrs_session_remove_variable,
        linkrs_session_set_variable,
    };

    let name = CString::new("x").expect("valid name");
    let invalid = CString::new("{not json").expect("valid cstring");

    assert_eq!(
        unsafe { linkrs_session_set_variable(ptr::null_mut(), name.as_ptr(), invalid.as_ptr()) }
            as i32,
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );
    assert_eq!(
        unsafe { linkrs_session_set_variable(ptr::null_mut(), ptr::null(), invalid.as_ptr()) }
            as i32,
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );

    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);
    assert_eq!(
        unsafe { linkrs_session_set_variable(session.handle(), name.as_ptr(), invalid.as_ptr()) },
        linkrs_error_code_t::LINKRS_ERROR as i32
    );
    assert_eq!(
        unsafe { linkrs_session_remove_variable(session.handle(), ptr::null()) },
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );
    assert!(unsafe { linkrs_session_get_variable(session.handle(), ptr::null()) }.is_null());
    assert!(unsafe { linkrs_session_list_variables(ptr::null_mut()) }.is_null());
}

// ==================== Observability ====================

#[test]
fn test_c_api_health_and_metrics() {
    use linkrs::api::embedded::c_api::inspection::{linkrs_health_json, linkrs_metrics_text};

    let test_db = CApiTestDatabase::new();

    let health = unsafe { take_c_string(linkrs_health_json(test_db.handle())) }
        .expect("non-null health payload");
    let parsed: serde_json::Value = serde_json::from_str(&health).expect("valid health JSON");
    assert_eq!(
        parsed.get("status").and_then(|value| value.as_str()),
        Some("healthy")
    );

    let metrics = unsafe { take_c_string(linkrs_metrics_text(test_db.handle())) }
        .expect("non-null metrics text");
    assert!(!metrics.is_empty());

    assert!(unsafe { linkrs_health_json(ptr::null_mut()) }.is_null());
    assert!(unsafe { linkrs_metrics_text(ptr::null_mut()) }.is_null());
}

// ==================== Schema Version History ====================

#[test]
fn test_c_api_schema_version_history_json() {
    use linkrs::api::embedded::c_api::inspection::linkrs_schema_version_history_json;

    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);
    let _ = CApiTestResult::from_query(&session, "CREATE SPACE demo (vid_type=STRING)");
    let _ = CApiTestResult::from_query(&session, "USE demo");
    let _ = CApiTestResult::from_query(&session, "CREATE TAG person(name string)");

    let space = CString::new("demo").expect("valid space");
    let label = CString::new("person").expect("valid label");

    let payload = unsafe {
        take_c_string(linkrs_schema_version_history_json(
            test_db.handle(),
            space.as_ptr(),
            label.as_ptr(),
            0,
        ))
    }
    .expect("non-null history payload");
    let parsed: serde_json::Value = serde_json::from_str(&payload).expect("valid history JSON");
    assert!(parsed.get("versions").is_some());

    // An unknown label surfaces as an error through the C API channel.
    let missing = CString::new("Missing").expect("valid label");
    let unknown = unsafe {
        linkrs_schema_version_history_json(test_db.handle(), space.as_ptr(), missing.as_ptr(), 0)
    };
    assert!(unknown.is_null());

    let rc_value = unsafe {
        linkrs_schema_version_history_json(test_db.handle(), ptr::null(), label.as_ptr(), 0)
    };
    assert!(rc_value.is_null());
    let rc_value = unsafe {
        linkrs_schema_version_history_json(test_db.handle(), space.as_ptr(), ptr::null(), 0)
    };
    assert!(rc_value.is_null());
}

// ==================== Transaction Inspection ====================

#[test]
fn test_c_api_transaction_inspection() {
    use linkrs::api::embedded::c_api::inspection::{
        linkrs_transaction_metrics_json, linkrs_transactions_json,
    };

    let test_db = CApiTestDatabase::new();
    let session = CApiTestSession::from_db(&test_db);
    let txn = CApiTestTransaction::from_session(&session);

    let list = unsafe { take_c_string(linkrs_transactions_json(test_db.handle())) }
        .expect("non-null transaction list");
    let parsed: serde_json::Value = serde_json::from_str(&list).expect("valid JSON array");
    let entries = parsed.as_array().expect("array");
    assert!(entries
        .iter()
        .any(|entry| entry.get("transaction_id").is_some()));

    let metrics = unsafe { take_c_string(linkrs_transaction_metrics_json(test_db.handle())) }
        .expect("non-null transaction metrics");
    let parsed: serde_json::Value = serde_json::from_str(&metrics).expect("valid JSON object");
    assert!(parsed.get("active").is_some());

    assert!(unsafe { linkrs_transactions_json(ptr::null_mut()) }.is_null());
    assert!(unsafe { linkrs_transaction_metrics_json(ptr::null_mut()) }.is_null());

    txn.commit();
}

// ==================== Bulk Configuration ====================

#[test]
fn test_c_api_bulk_update_config() {
    use linkrs::api::embedded::c_api::config::{
        linkrs_bulk_update_config, linkrs_config_free, linkrs_config_memory,
    };

    let config = unsafe { linkrs_config_memory() };
    assert!(!config.is_null());

    let updates =
        CString::new(r#"{"cache_size_mb":128,"timeout_ms":5000,"sync_mode":2,"read_only":true}"#)
            .expect("valid JSON");
    assert_eq!(
        unsafe { linkrs_bulk_update_config(config, updates.as_ptr()) },
        linkrs_error_code_t::LINKRS_OK as i32
    );

    let unknown = CString::new(r#"{"unknown_key":1}"#).expect("valid JSON");
    assert_eq!(
        unsafe { linkrs_bulk_update_config(config, unknown.as_ptr()) },
        linkrs_error_code_t::LINKRS_ERROR as i32
    );

    let invalid = CString::new(r#"{"cache_size_mb":"big"}"#).expect("valid JSON");
    assert_eq!(
        unsafe { linkrs_bulk_update_config(config, invalid.as_ptr()) },
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );

    assert_eq!(
        unsafe { linkrs_bulk_update_config(ptr::null_mut(), updates.as_ptr()) },
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );
    assert_eq!(
        unsafe { linkrs_bulk_update_config(config, ptr::null()) },
        linkrs_error_code_t::LINKRS_MISUSE as i32
    );

    unsafe { linkrs_config_free(config) };
}

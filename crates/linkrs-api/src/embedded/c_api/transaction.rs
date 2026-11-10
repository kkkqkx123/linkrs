//! C API Transaction Management Module
//!
//! Provides transaction management functionality, including transaction start, commit, rollback, and savepoints

use crate::api_core::types::TransactionHandle;
use crate::embedded::c_api::database::LinkrsHandle;
use crate::embedded::c_api::error::{
    error_code_from_core_error, linkrs_error_code_t, set_last_error_message,
};
use crate::embedded::c_api::query::{collect_named_params, convert_c_value_to_rust};
use crate::embedded::c_api::result::LinkrsResultHandle;
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{
    linkrs_result_t, linkrs_session_t, linkrs_t, linkrs_txn_t, linkrs_value_t,
};
use linkrs_core::Value;
use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

/// Internal structure of transaction handles
///
/// Note: This structure holds the session pointer, but does not own the session.
/// The caller must ensure that the session is not closed until the transaction completes.
pub struct LinkrsTxnHandle {
    pub(crate) session: *mut LinkrsSessionHandle,
    pub(crate) txn_handle: Option<TransactionHandle>,
    pub(crate) committed: bool,
    pub(crate) rolled_back: bool,
}

impl LinkrsTxnHandle {
    /// Check if the session is still active
    fn is_session_valid(&self) -> bool {
        !self.session.is_null()
    }

    /// Get session reference (if valid)
    fn get_session(&self) -> Option<&LinkrsSessionHandle> {
        if self.is_session_valid() {
            Some(unsafe { &*self.session })
        } else {
            None
        }
    }
}

impl Drop for LinkrsTxnHandle {
    fn drop(&mut self) {
        if !self.committed && !self.rolled_back {
            if let Some(txn_handle) = self.txn_handle.take() {
                // Try to rollback the transaction if not committed/rolled back
                if let Some(session) = self.get_session() {
                    let _ = session.inner.rollback_transaction(txn_handle);
                }
            }
        }
    }
}

/// Begin a transaction
///
/// # Parameters
/// - `session`: session handle
/// - `txn`: output parameter, transaction handle
///
/// # Return
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `txn` must be a valid pointer to store the transaction handle
/// - The session must not have been closed
/// - The caller is responsible for freeing the transaction using `linkrs_txn_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_begin(
    session: *mut linkrs_session_t,
    txn: *mut *mut linkrs_txn_t,
) -> c_int {
    if session.is_null() || txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &*(session as *mut LinkrsSessionHandle);

    // Use embedded session API instead of direct TransactionManager access
    match handle.inner.begin_transaction() {
        Ok(transaction) => {
            let txn_handle = transaction.txn_handle();
            let txn_handle_box = Box::new(LinkrsTxnHandle {
                session: session as *mut LinkrsSessionHandle,
                txn_handle: Some(txn_handle),
                committed: false,
                rolled_back: false,
            });
            // Leak the transaction to prevent Drop from being called
            // The transaction lifecycle is now managed by LinkrsTxnHandle
            std::mem::forget(transaction);
            *txn = Box::into_raw(txn_handle_box) as *mut linkrs_txn_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let error_code = linkrs_error_code_t::LINKRS_ABORT as c_int;
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            *txn = ptr::null_mut();
            error_code
        }
    }
}

/// Starting a read-only transaction
///
/// # Parameters
/// - `session`: Session handle
/// - `txn`: Output parameter, transaction handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `txn` must be a valid pointer to store the transaction handle
/// - The session must not have been closed
/// - The caller is responsible for freeing the transaction using `linkrs_txn_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_begin_readonly(
    session: *mut linkrs_session_t,
    txn: *mut *mut linkrs_txn_t,
) -> c_int {
    if session.is_null() || txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &*(session as *mut LinkrsSessionHandle);

    // Use embedded session API with read-only config
    let config = crate::embedded::TransactionConfig::new().read_only();
    match handle.inner.begin_transaction_with_config(config) {
        Ok(transaction) => {
            let txn_handle = transaction.txn_handle();
            let txn_handle_box = Box::new(LinkrsTxnHandle {
                session: session as *mut LinkrsSessionHandle,
                txn_handle: Some(txn_handle),
                committed: false,
                rolled_back: false,
            });
            std::mem::forget(transaction);
            *txn = Box::into_raw(txn_handle_box) as *mut linkrs_txn_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let error_code = linkrs_error_code_t::LINKRS_ABORT as c_int;
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            *txn = ptr::null_mut();
            error_code
        }
    }
}

/// Executing queries in a transaction
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `query`: Query statement (UTF-8 encoding)
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - The transaction must not have been committed or rolled back
/// - The caller is responsible for freeing the result using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_execute(
    txn: *mut linkrs_txn_t,
    query: *const c_char,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if txn.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Checking session validity
    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    let ctx = crate::api_core::QueryRequest {
        space_id: session.inner.space_id(),
        space_name: session.inner.space_name().map(|s| s.to_string()),
        auto_commit: false,
        transaction_id: Some(txn_handle.0),
        parameters: None,
        session_variables: None,
        query_id: None,
        isolation_level: None,
        parsed_statement: None,
        consistency: Default::default(),
    };

    let mut query_api = session.inner.query_api_mut();
    match query_api.execute(query_str, ctx) {
        Ok(core_result) => {
            let query_result = crate::embedded::result::QueryResult::from_core(core_result);
            let result_handle = Box::new(LinkrsResultHandle {
                inner: query_result,
            });
            *result = Box::into_raw(result_handle) as *mut linkrs_result_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            *result = ptr::null_mut();
            error_code
        }
    }
}

/// Executing a parameterized query in a transaction
///
/// Positional binding mirrors `linkrs_execute_params`: `params[i]` binds to
/// the `@param_{i}` query parameter. The transaction state machine is
/// unchanged: calling on a committed or rolled back transaction fails with
/// LINKRS_MISUSE.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `query`: Query statement (UTF-8 encoding)
/// - `params`: Parameter array (may be NULL when `param_count` is 0)
/// - `param_count`: Number of parameters
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - If `params` is not NULL, it must point to at least `param_count` valid `linkrs_value_t` elements
/// - The transaction must not have been committed or rolled back
/// - The caller is responsible for freeing the result using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_execute_params(
    txn: *mut linkrs_txn_t,
    query: *const c_char,
    params: *const linkrs_value_t,
    param_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if txn.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    let mut params_map: HashMap<String, Value> = HashMap::new();
    if !params.is_null() && param_count > 0 {
        for i in 0..param_count {
            let param = &*params.add(i);
            let param_name = format!("param_{i}");
            let value = convert_c_value_to_rust(param);
            params_map.insert(param_name, value);
        }
    }

    let ctx = crate::api_core::QueryRequest {
        space_id: session.inner.space_id(),
        space_name: session.inner.space_name().map(|s| s.to_string()),
        auto_commit: false,
        transaction_id: Some(txn_handle.0),
        parameters: Some(params_map),
        session_variables: None,
        query_id: None,
        isolation_level: None,
        parsed_statement: None,
        consistency: Default::default(),
    };

    let mut query_api = session.inner.query_api_mut();
    match query_api.execute(query_str, ctx) {
        Ok(core_result) => {
            let query_result = crate::embedded::result::QueryResult::from_core(core_result);
            let result_handle = Box::new(LinkrsResultHandle {
                inner: query_result,
            });
            *result = Box::into_raw(result_handle) as *mut linkrs_result_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{e}");
            set_last_error_message(error_msg);
            *result = ptr::null_mut();
            error_code
        }
    }
}

/// Commit transactions
///
/// # Parameters
/// - `txn`: Transaction handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - The transaction must not have been committed or rolled back already
/// - The associated session must still be valid
/// - After calling this function, the transaction handle should be freed using `linkrs_txn_free`
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_commit(txn: *mut linkrs_txn_t) -> c_int {
    if txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Check session validity first (only check pointer, don't borrow)
    if !handle.is_session_valid() {
        set_last_error_message("associated session is no longer valid".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Get session pointer for later use
    let session_ptr = handle.session;

    // NOTE: the C commit hook is bridged into the transaction manager's
    // veto registry at `linkrs_commit_hook` time, so evaluation happens
    // inside `commit_transaction` below (shared point with Rust
    // `register_commit_veto` hooks). No direct invocation here.

    // Use embedded session API instead of direct TransactionManager access
    let session = &*session_ptr;

    // Commit transaction (synchronous, no longer async). A veto fails with
    // `CommitVetoed` and leaves the transaction active; roll it back
    // through the standard path so the rollback hook fires, exactly like
    // the historical pre-commit veto behavior.
    let txn_id = match handle.txn_handle.as_ref() {
        Some(h) => h.0,
        None => return linkrs_error_code_t::LINKRS_INTERNAL as c_int,
    };
    let result = session.inner.txn_manager().commit_transaction(txn_id);
    match result {
        Ok(_) => {
            handle.txn_handle.take();
            handle.committed = true;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) if e.kind() == linkrs_transaction::TransactionErrorKind::CommitVetoed => {
            linkrs_txn_rollback(txn)
        }
        Err(e) => {
            handle.txn_handle.take();
            let error_code = linkrs_error_code_t::LINKRS_ABORT as c_int;
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            error_code
        }
    }
}

/// Rolling back transactions
///
/// # Parameters
/// - `txn`: Transaction handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - The transaction must not have been committed or rolled back already
/// - The associated session must still be valid
/// - After calling this function, the transaction handle should be freed using `linkrs_txn_free`
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_rollback(txn: *mut linkrs_txn_t) -> c_int {
    if txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Check session validity first (only check pointer, don't borrow)
    if !handle.is_session_valid() {
        set_last_error_message("associated session is no longer valid".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Get session pointer for later use
    let session_ptr = handle.session;

    // Execute rollback hook if present, with panic isolation so a broken
    // observer can never block the rollback itself.
    {
        let session = &*session_ptr;
        if let Some(callback) = session.rollback_hook {
            let user_data = session.rollback_hook_user_data;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                callback(user_data);
            }))
            .is_err()
            {
                log::error!("rollback hook panicked; continuing rollback");
            }
        }
    }

    let txn_handle = match handle.txn_handle.take() {
        Some(h) => h,
        None => return linkrs_error_code_t::LINKRS_INTERNAL as c_int,
    };

    // Use embedded session API instead of direct TransactionManager access
    let session = &*session_ptr;
    match session.inner.rollback_transaction(txn_handle) {
        Ok(_) => {
            handle.rolled_back = true;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let error_code = linkrs_error_code_t::LINKRS_ABORT as c_int;
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            error_code
        }
    }
}

/// Creating a savepoint
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `name`: Name of the savepoint (UTF-8 encoding)
///
/// # Returns
/// - Success: Savepoint ID
/// - Failure: -1
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_savepoint(txn: *mut linkrs_txn_t, name: *const c_char) -> i64 {
    // No status code travels with the id, so every failure records the
    // thread-local detail for the caller.
    if txn.is_null() || name.is_null() {
        set_last_error_message("null transaction handle or savepoint name".to_string());
        return -1;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_last_error_message("savepoint name is not valid UTF-8".to_string());
            return -1;
        }
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return -1;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return -1;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return -1;
        }
    };

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.create_savepoint(txn_handle, name_str) {
        Ok(id) => id.0 as i64,
        Err(e) => {
            set_last_error_message(format!("{e}"));
            -1
        }
    }
}

/// Release the savepoint.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `savepoint_id`: ID of the savepoint
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `savepoint_id` must be a valid savepoint ID returned by `linkrs_txn_savepoint`
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_release_savepoint(
    txn: *mut linkrs_txn_t,
    savepoint_id: i64,
) -> c_int {
    if txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let savepoint = crate::api_core::types::SavepointId(savepoint_id as u64);

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.release_savepoint(txn_handle, savepoint) {
        Ok(_) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let core_error = crate::api_core::error::CoreError::TransactionFailed(format!("{}", e));
            let (error_code, _) = error_code_from_core_error(&core_error);
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            error_code
        }
    }
}

/// Roll back to the savepoint.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `savepoint_id`: Savepoint ID
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `savepoint_id` must be a valid savepoint ID returned by `linkrs_txn_savepoint`
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_rollback_to_savepoint(
    txn: *mut linkrs_txn_t,
    savepoint_id: i64,
) -> c_int {
    if txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut LinkrsTxnHandle);

    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    let savepoint = crate::api_core::types::SavepointId(savepoint_id as u64);

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.rollback_to_savepoint(txn_handle, savepoint) {
        Ok(_) => linkrs_error_code_t::LINKRS_OK as c_int,
        Err(e) => {
            let core_error = crate::api_core::error::CoreError::TransactionFailed(format!("{}", e));
            let (error_code, _) = error_code_from_core_error(&core_error);
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            error_code
        }
    }
}

/// Free the transaction handle
///
/// # Parameters
/// - `txn`: Transaction handle
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `txn` can be null (in which case this function does nothing)
/// - After calling this function, the handle is invalid and must not be used again
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_free(txn: *mut linkrs_txn_t) {
    if !txn.is_null() {
        let _ = Box::from_raw(txn as *mut LinkrsTxnHandle);
    }
}

/// List currently active transactions (`Database::list_transactions` over FFI).
///
/// Returns a heap JSON array; each entry carries the same fields as the
/// HTTP `GET /v1/transactions` response (`transaction_id`, `state`,
/// `owner`, `read_timestamp`, `write_timestamp`, `elapsed_ms`,
/// `last_activity_ms`, `rollback_only`, `staged_bytes`, `undo_bytes`,
/// `blocking_reason`).
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - JSON array string (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_transaction_list(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }

    let handle = &*(db as *mut LinkrsHandle);
    let transactions: Vec<serde_json::Value> = handle
        .inner
        .list_transactions()
        .iter()
        .map(transaction_info_json)
        .collect();

    match serde_json::to_string(&transactions) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => c_json.into_raw(),
            Err(_) => ptr::null_mut(),
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize transaction list: {e}"));
            ptr::null_mut()
        }
    }
}

/// Snapshot transaction counters and resource gauges
/// (`Database::transaction_metrics` over FFI).
///
/// Returns a heap JSON object with the `TransactionMetrics` fields:
/// `begun`, `active`, `committed`, `aborted`, `active_statements`,
/// `active_snapshots`, `pending_writes`, `staged_wal_bytes`, `undo_bytes`.
///
/// # Arguments
/// - `db`: Database handle
///
/// # Returns
/// - JSON object string (UTF-8 encoded), returns NULL on error
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `db` must be a valid database handle created by `linkrs_open` or `linkrs_open_with_config`
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_transaction_metrics(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }

    let handle = &*(db as *mut LinkrsHandle);
    let metrics = handle.inner.transaction_metrics();
    match serde_json::to_string(&metrics) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => c_json.into_raw(),
            Err(_) => ptr::null_mut(),
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize transaction metrics: {e}"));
            ptr::null_mut()
        }
    }
}

/// Run a transaction query with named parameters and optional session variables.
///
/// Shares the state machine of `linkrs_txn_execute_params`: calling on a
/// committed or rolled back transaction fails with LINKRS_MISUSE.
/// `variables` fills the request's session-variable channel, so `$name`
/// references resolve from the caller map instead of the session snapshot.
unsafe fn run_txn_named_execution(
    handle: &mut LinkrsTxnHandle,
    query_str: &str,
    params: HashMap<String, Value>,
    variables: Option<HashMap<String, Value>>,
    out_result: *mut *mut linkrs_result_t,
) -> c_int {
    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    let ctx = crate::api_core::QueryRequest {
        space_id: session.inner.space_id(),
        space_name: session.inner.space_name().map(|s| s.to_string()),
        auto_commit: false,
        transaction_id: Some(txn_handle.0),
        parameters: Some(params),
        session_variables: variables,
        query_id: None,
        isolation_level: None,
        parsed_statement: None,
        consistency: Default::default(),
    };

    let mut query_api = session.inner.query_api_mut();
    match query_api.execute(query_str, ctx) {
        Ok(core_result) => {
            let query_result = crate::embedded::result::QueryResult::from_core(core_result);
            let result_handle = Box::new(LinkrsResultHandle {
                inner: query_result,
            });
            *out_result = Box::into_raw(result_handle) as *mut linkrs_result_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let (error_code, _) = error_code_from_core_error(&e);
            let error_msg = format!("{e}");
            set_last_error_message(error_msg);
            *out_result = ptr::null_mut();
            error_code
        }
    }
}

/// Execute a named-parameter query in a transaction.
///
/// Named binding mirrors `linkrs_execute_named_params`: `names[i]` (without
/// the `@` sigil) fills `@names[i]` in the statement. The transaction state
/// machine is unchanged: calling on a committed or rolled back transaction
/// fails with LINKRS_MISUSE.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `query`: Query statement (UTF-8 encoding)
/// - `names`: Parameter name array (may be NULL when `param_count` is 0)
/// - `values`: Parameter value array (may be NULL when `param_count` is 0)
/// - `param_count`: Number of parameters
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - When `param_count` is nonzero, `names` must point to at least
///   `param_count` valid name pointers and `values` to at least
///   `param_count` valid `linkrs_value_t` elements
/// - The transaction must not have been committed or rolled back
/// - The caller is responsible for freeing the result using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_execute_named_params(
    txn: *mut linkrs_txn_t,
    query: *const c_char,
    names: *const *const c_char,
    values: *const linkrs_value_t,
    param_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if txn.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let params = match collect_named_params(names, values, param_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);
    run_txn_named_execution(handle, query_str, params, None, result)
}

/// Execute a query with named parameters and session variables in a transaction.
///
/// Projection of the session-level `linkrs_execute_with_variables` onto an
/// explicit transaction: `@name` references resolve from the parameter map
/// and `$name` references from the variable map, independently.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `query`: Query statement (UTF-8 encoding)
/// - `param_names`: Parameter name array (may be NULL when `param_count` is 0)
/// - `params`: Parameter value array (may be NULL when `param_count` is 0)
/// - `param_count`: Number of parameters
/// - `var_names`: Session variable name array (may be NULL when `var_count` is 0)
/// - `vars`: Session variable value array (may be NULL when `var_count` is 0)
/// - `var_count`: Number of session variables
/// - `result`: Output parameter, result set handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin` or `linkrs_txn_begin_readonly`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - Each name array must point to at least its count of valid name
///   pointers (or be NULL when the matching count is 0), and each value
///   array to at least its count of valid `linkrs_value_t` elements
/// - The transaction must not have been committed or rolled back
/// - The caller is responsible for freeing the result using `linkrs_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_execute_with_variables(
    txn: *mut linkrs_txn_t,
    query: *const c_char,
    param_names: *const *const c_char,
    params: *const linkrs_value_t,
    param_count: usize,
    var_names: *const *const c_char,
    vars: *const linkrs_value_t,
    var_count: usize,
    result: *mut *mut linkrs_result_t,
) -> c_int {
    if txn.is_null() || query.is_null() || result.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let params_map = match collect_named_params(param_names, params, param_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let vars_map = match collect_named_params(var_names, vars, var_count) {
        Some(map) => map,
        None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);
    run_txn_named_execution(handle, query_str, params_map, Some(vars_map), result)
}

/// Serialize one transaction entry, shared by the list and info entry points.
///
/// The shape matches the HTTP `GET /v1/transactions` response so C
/// callers and HTTP callers observe the same fields.
fn transaction_info_json(info: &linkrs_transaction::TransactionInfo) -> serde_json::Value {
    serde_json::json!({
        "transaction_id": info.id.as_u64(),
        "state": info.state.to_string(),
        "owner": info.owner,
        "read_timestamp": info.read_timestamp,
        "write_timestamp": info.write_timestamp,
        "elapsed_ms": info.elapsed.as_millis(),
        "last_activity_ms": info.last_activity.as_millis(),
        "rollback_only": info.rollback_only,
        "staged_bytes": info.staged_bytes,
        "undo_bytes": info.undo_bytes,
        "blocking_reason": info.blocking_reason,
    })
}

/// Parse the JSON document accepted by `linkrs_txn_begin_with_config`.
///
/// All keys are optional: `read_only` (bool), `timeout_ms`,
/// `query_timeout_ms`, `statement_timeout_ms`, `idle_timeout_ms`
/// (nonnegative integers, 0 or absent means unset), `durability`
/// (`"none"` / `"async"` / `"sync"`, case-insensitive, default sync) and
/// `isolation` (`"repeatable_read"` / `"read_committed"` /
/// `"serializable"`, case-insensitive, default repeatable read).
fn parse_txn_config_json(raw: &str) -> Result<crate::embedded::TransactionConfig, String> {
    use linkrs_transaction::{DurabilityLevel, IsolationLevel};

    let doc: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("invalid transaction config: {e}"))?;
    let get = |key: &str| doc.get(key);
    let mut config = crate::embedded::TransactionConfig::new();

    if let Some(v) = get("read_only") {
        config.read_only = v
            .as_bool()
            .ok_or_else(|| "transaction config 'read_only' must be a boolean".to_string())?;
    }
    let timeout = |key: &str| -> Result<Option<std::time::Duration>, String> {
        match get(key) {
            None => Ok(None),
            Some(v) => {
                let ms = v.as_u64().ok_or_else(|| {
                    format!("transaction config '{key}' must be a nonnegative integer")
                })?;
                Ok((ms > 0).then(|| std::time::Duration::from_millis(ms)))
            }
        }
    };
    config.timeout = timeout("timeout_ms")?;
    config.query_timeout = timeout("query_timeout_ms")?;
    config.statement_timeout = timeout("statement_timeout_ms")?;
    config.idle_timeout = timeout("idle_timeout_ms")?;
    if let Some(v) = get("durability") {
        let name = v
            .as_str()
            .ok_or_else(|| "transaction config 'durability' must be a string".to_string())?;
        config.durability = match name.to_ascii_lowercase().as_str() {
            "none" => DurabilityLevel::None,
            "async" => DurabilityLevel::Async,
            "sync" => DurabilityLevel::Sync,
            _ => return Err(format!("unknown durability level: {name}")),
        };
    }
    if let Some(v) = get("isolation") {
        let name = v
            .as_str()
            .ok_or_else(|| "transaction config 'isolation' must be a string".to_string())?;
        config.isolation_level = match name.to_ascii_lowercase().as_str() {
            "repeatable_read" => IsolationLevel::RepeatableRead,
            "read_committed" => IsolationLevel::ReadCommitted,
            "serializable" => IsolationLevel::Serializable,
            _ => return Err(format!("unknown isolation level: {name}")),
        };
    }
    Ok(config)
}

/// Begin a transaction with explicit options.
///
/// `config_json` carries the `TransactionConfig` fields documented on the
/// parser above; a NULL pointer selects the defaults (same as
/// `linkrs_txn_begin`). Unknown durability or isolation names fail with
/// LINKRS_MISUSE and leave `*txn` NULL.
///
/// # Parameters
/// - `session`: Session handle
/// - `config_json`: JSON option document (UTF-8 encoding) or NULL for defaults
/// - `txn`: Output parameter, transaction handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `config_json` must be NULL or a valid pointer to a null-terminated UTF-8 string
/// - `txn` must be a valid pointer to store the transaction handle
/// - The session must not have been closed
/// - The caller is responsible for freeing the transaction using `linkrs_txn_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_begin_with_config(
    session: *mut linkrs_session_t,
    config_json: *const c_char,
    txn: *mut *mut linkrs_txn_t,
) -> c_int {
    if session.is_null() || txn.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *txn = ptr::null_mut();

    let config = if config_json.is_null() {
        crate::embedded::TransactionConfig::new()
    } else {
        match CStr::from_ptr(config_json).to_str() {
            Ok(raw) => match parse_txn_config_json(raw) {
                Ok(config) => config,
                Err(msg) => {
                    set_last_error_message(msg);
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
    };

    let handle = &*(session as *mut LinkrsSessionHandle);
    match handle.inner.begin_transaction_with_config(config) {
        Ok(transaction) => {
            let txn_handle = transaction.txn_handle();
            let txn_handle_box = Box::new(LinkrsTxnHandle {
                session: session as *mut LinkrsSessionHandle,
                txn_handle: Some(txn_handle),
                committed: false,
                rolled_back: false,
            });
            std::mem::forget(transaction);
            *txn = Box::into_raw(txn_handle_box) as *mut linkrs_txn_t;
            linkrs_error_code_t::LINKRS_OK as c_int
        }
        Err(e) => {
            let error_code = linkrs_error_code_t::LINKRS_ABORT as c_int;
            let error_msg = format!("{}", e);
            set_last_error_message(error_msg);
            error_code
        }
    }
}

/// Describe one live transaction as JSON.
///
/// The object carries the same fields as one entry of
/// `linkrs_transaction_list` (`transaction_id`, `state`, `owner`,
/// `read_timestamp`, `write_timestamp`, `elapsed_ms`, `last_activity_ms`,
/// `rollback_only`, `staged_bytes`, `undo_bytes`, `blocking_reason`).
/// Querying a committed or rolled back handle fails with LINKRS_MISUSE; a
/// handle whose transaction already vanished fails with LINKRS_NOTFOUND.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `out_json`: Output parameter, heap JSON object string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_json` holds the JSON object)
/// - Failure: Error code (`*out_json` is NULL)
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin`,
///   `linkrs_txn_begin_readonly` or `linkrs_txn_begin_with_config`
/// - `out_json` must be a valid pointer to store the result
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_info(
    txn: *mut linkrs_txn_t,
    out_json: *mut *mut c_char,
) -> c_int {
    if txn.is_null() || out_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *out_json = ptr::null_mut();

    let handle = &mut *(txn as *mut LinkrsTxnHandle);
    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };
    let txn_id = match handle.txn_handle.as_ref() {
        Some(h) => h.0,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    match session
        .inner
        .list_transactions()
        .iter()
        .find(|info| info.id == txn_id)
    {
        Some(info) => match serde_json::to_string(&transaction_info_json(info)) {
            Ok(json) => match CString::new(json) {
                Ok(c_json) => {
                    *out_json = c_json.into_raw();
                    linkrs_error_code_t::LINKRS_OK as c_int
                }
                Err(_) => linkrs_error_code_t::LINKRS_ERROR as c_int,
            },
            Err(e) => {
                set_last_error_message(format!("failed to serialize transaction info: {e}"));
                linkrs_error_code_t::LINKRS_ERROR as c_int
            }
        },
        None => {
            set_last_error_message("transaction not found".to_string());
            linkrs_error_code_t::LINKRS_NOTFOUND as c_int
        }
    }
}

/// List the active savepoints of a transaction as JSON.
///
/// Returns a heap JSON array; each entry carries `id` (the value
/// `linkrs_txn_savepoint` returns), `name` (null for unnamed savepoints)
/// and `sequence` (creation order). An empty array means no savepoint is
/// currently active.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `out_json`: Output parameter, heap JSON array string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_json` holds the JSON array)
/// - Failure: Error code (`*out_json` is NULL)
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin`,
///   `linkrs_txn_begin_readonly` or `linkrs_txn_begin_with_config`
/// - `out_json` must be a valid pointer to store the result
/// - The transaction must not have been committed or rolled back
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_list_savepoints(
    txn: *mut linkrs_txn_t,
    out_json: *mut *mut c_char,
) -> c_int {
    if txn.is_null() || out_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *out_json = ptr::null_mut();

    let handle = &mut *(txn as *mut LinkrsTxnHandle);
    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        }
    };
    let txn_id = match handle.txn_handle.as_ref() {
        Some(h) => h.0,
        None => {
            set_last_error_message("no active transaction".to_string());
            return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
        }
    };

    let savepoints: Vec<serde_json::Value> = session
        .inner
        .txn_manager()
        .get_active_savepoints(txn_id)
        .into_iter()
        .map(|sp| {
            serde_json::json!({
                "id": sp.id,
                "name": sp.name,
                "sequence": sp.sequence,
            })
        })
        .collect();
    match serde_json::to_string(&savepoints) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => {
                *out_json = c_json.into_raw();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(_) => linkrs_error_code_t::LINKRS_ERROR as c_int,
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize savepoints: {e}"));
            linkrs_error_code_t::LINKRS_ERROR as c_int
        }
    }
}

/// Find a savepoint by name.
///
/// Savepoint names are unique per transaction (duplicates are rejected at
/// creation), so at most one entry matches.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `name`: Savepoint name (UTF-8 encoding)
///
/// # Returns
/// - Savepoint ID on success, -1 when the transaction is finished, the
///   session is gone, or no savepoint carries this name
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `linkrs_txn_begin`,
///   `linkrs_txn_begin_readonly` or `linkrs_txn_begin_with_config`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn linkrs_txn_find_savepoint(
    txn: *mut linkrs_txn_t,
    name: *const c_char,
) -> i64 {
    if txn.is_null() || name.is_null() {
        return -1;
    }
    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let handle = &mut *(txn as *mut LinkrsTxnHandle);
    if handle.committed || handle.rolled_back {
        set_last_error_message("transaction already completed".to_string());
        return -1;
    }
    let session = match handle.get_session() {
        Some(s) => s,
        None => {
            set_last_error_message("associated session is no longer valid".to_string());
            return -1;
        }
    };
    let txn_id = match handle.txn_handle.as_ref() {
        Some(h) => h.0,
        None => {
            set_last_error_message("no active transaction".to_string());
            return -1;
        }
    };

    session
        .inner
        .txn_manager()
        .get_active_savepoints(txn_id)
        .into_iter()
        .find(|sp| sp.name.as_deref() == Some(name_str))
        .map(|sp| sp.id as i64)
        .unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::database::linkrs_close;

    #[test]
    fn test_txn_named_params_and_variables() {
        use crate::embedded::c_api::result::{linkrs_get_int_by_index, linkrs_result_free};
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
        use crate::embedded::c_api::types::{
            linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
        };

        fn int_value(v: i64) -> linkrs_value_t {
            linkrs_value_t {
                type_: linkrs_value_type_t::LINKRS_INT,
                data: linkrs_value_data_t { integer: v },
            }
        }

        unsafe {
            let db = open_memory_db();
            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin(session, &mut txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let x_name = CString::new("x").unwrap();
            let v_name = CString::new("v").unwrap();
            let param_names = [x_name.as_ptr()];
            let var_names = [v_name.as_ptr()];
            let param = int_value(40);
            let var = int_value(2);
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            let query = CString::new("RETURN @x").unwrap();
            assert_eq!(
                linkrs_txn_execute_named_params(
                    txn,
                    query.as_ptr(),
                    param_names.as_ptr(),
                    &param,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut got: i64 = 0;
            assert_eq!(
                linkrs_get_int_by_index(result, 0, 0, &mut got),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(got, 40);
            linkrs_result_free(result);

            let query = CString::new("RETURN @x + $v").unwrap();
            assert_eq!(
                linkrs_txn_execute_with_variables(
                    txn,
                    query.as_ptr(),
                    param_names.as_ptr(),
                    &param,
                    1,
                    var_names.as_ptr(),
                    &var,
                    1,
                    &mut result
                ),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut got: i64 = 0;
            assert_eq!(
                linkrs_get_int_by_index(result, 0, 0, &mut got),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(got, 42);
            linkrs_result_free(result);

            assert_eq!(
                linkrs_txn_rollback(txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            linkrs_txn_free(txn);
            linkrs_session_close(session);
            assert_eq!(linkrs_close(db), 0);
        }
    }

    #[test]
    fn test_txn_begin_with_config_info_and_savepoints() {
        use crate::embedded::c_api::database::linkrs_free_string;
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};

        unsafe {
            let db = open_memory_db();
            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            // NULL config selects the defaults.
            let mut txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin_with_config(session, ptr::null(), &mut txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!txn.is_null());

            let mut info: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_txn_info(txn, &mut info),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!info.is_null());
            let text = CStr::from_ptr(info).to_str().expect("utf8").to_string();
            linkrs_free_string(info);
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("info json");
            assert!(parsed.get("transaction_id").is_some());
            assert!(parsed.get("state").is_some());
            assert!(parsed.get("staged_bytes").is_some());

            // No savepoints yet: empty array, unknown name misses.
            let mut list: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_txn_list_savepoints(txn, &mut list),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let text = CStr::from_ptr(list).to_str().expect("utf8").to_string();
            linkrs_free_string(list);
            assert_eq!(text, "[]");
            let missing = CString::new("nope").unwrap();
            assert_eq!(linkrs_txn_find_savepoint(txn, missing.as_ptr()), -1);
            assert_eq!(
                linkrs_txn_find_savepoint(ptr::null_mut(), missing.as_ptr()),
                -1
            );

            let sp_name = CString::new("sp1").unwrap();
            let sp_id = linkrs_txn_savepoint(txn, sp_name.as_ptr());
            assert!(sp_id >= 0);
            assert_eq!(linkrs_txn_find_savepoint(txn, sp_name.as_ptr()), sp_id);

            let mut list: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_txn_list_savepoints(txn, &mut list),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let text = CStr::from_ptr(list).to_str().expect("utf8").to_string();
            linkrs_free_string(list);
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&text).expect("sp json");
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].get("name").and_then(|v| v.as_str()), Some("sp1"));

            assert_eq!(
                linkrs_txn_rollback(txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            // Finished handles reject info queries.
            let mut info: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_txn_info(txn, &mut info),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert!(info.is_null());
            linkrs_txn_free(txn);

            // Read-only JSON config opens a read-only transaction.
            let cfg = CString::new(
                r#"{"read_only": true, "timeout_ms": 5000, "durability": "sync", "isolation": "serializable"}"#,
            )
            .unwrap();
            let mut ro_txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin_with_config(session, cfg.as_ptr(), &mut ro_txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!ro_txn.is_null());
            assert_eq!(
                linkrs_txn_rollback(ro_txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            linkrs_txn_free(ro_txn);

            // Unknown option values fail without creating a handle.
            let bad = CString::new(r#"{"durability": "diamond"}"#).unwrap();
            let mut bad_txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin_with_config(session, bad.as_ptr(), &mut bad_txn),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert!(bad_txn.is_null());
            let bad = CString::new(r#"{"timeout_ms": -1}"#).unwrap();
            assert_eq!(
                linkrs_txn_begin_with_config(session, bad.as_ptr(), &mut bad_txn),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert!(bad_txn.is_null());
            assert_eq!(
                linkrs_txn_begin_with_config(ptr::null_mut(), ptr::null(), &mut bad_txn),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_txn_info(ptr::null_mut(), &mut info),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_txn_list_savepoints(ptr::null_mut(), &mut info),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            linkrs_session_close(session);
            assert_eq!(linkrs_close(db), 0);
        }
    }

    #[test]
    fn test_txn_begin_null_params() {
        let result = unsafe { linkrs_txn_begin(ptr::null_mut(), ptr::null_mut()) };
        assert_eq!(result, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_txn_free_null() {
        // Should not panic
        unsafe { linkrs_txn_free(ptr::null_mut()) };
    }

    #[test]
    fn test_txn_commit_null() {
        let result = unsafe { linkrs_txn_commit(ptr::null_mut()) };
        assert_eq!(result, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_txn_rollback_null() {
        let result = unsafe { linkrs_txn_rollback(ptr::null_mut()) };
        assert_eq!(result, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_txn_execute_params_positional_binding() {
        use crate::embedded::c_api::database::linkrs_close;
        use crate::embedded::c_api::result::{linkrs_get_int_by_index, linkrs_result_free};
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
        use crate::embedded::c_api::types::{
            linkrs_session_t, linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
        };

        unsafe {
            let db = open_memory_db();
            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin(session, &mut txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let query = CString::new("RETURN @param_0").unwrap();
            let param = linkrs_value_t {
                type_: linkrs_value_type_t::LINKRS_INT,
                data: linkrs_value_data_t { integer: 41 },
            };
            let mut result: *mut linkrs_result_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_execute_params(txn, query.as_ptr(), &param, 1, &mut result),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut value: i64 = 0;
            assert_eq!(
                linkrs_get_int_by_index(result, 0, 0, &mut value),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(value, 41);
            linkrs_result_free(result);

            assert_eq!(
                linkrs_txn_execute_params(ptr::null_mut(), query.as_ptr(), &param, 1, &mut result),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            assert_eq!(
                linkrs_txn_rollback(txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            linkrs_txn_free(txn);
            linkrs_session_close(session);
            assert_eq!(linkrs_close(db), 0);
        }
    }

    fn open_memory_db() -> *mut linkrs_t {
        use crate::embedded::c_api::config::linkrs_config_memory;
        use crate::embedded::c_api::database::linkrs_open_with_config;

        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(config, &mut db),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert!(!db.is_null());
            db
        }
    }

    #[test]
    fn test_transaction_list_empty_and_metrics() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_free_string};

        unsafe {
            let db = open_memory_db();

            let list = linkrs_transaction_list(db);
            assert!(!list.is_null());
            let json = CStr::from_ptr(list).to_str().expect("utf8").to_string();
            linkrs_free_string(list);
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("list json");
            assert_eq!(parsed, serde_json::json!([]));

            let metrics = linkrs_transaction_metrics(db);
            assert!(!metrics.is_null());
            let json = CStr::from_ptr(metrics).to_str().expect("utf8").to_string();
            linkrs_free_string(metrics);
            let parsed: serde_json::Value = serde_json::from_str(&json).expect("metrics json");
            assert!(parsed.get("begun").is_some());
            assert!(parsed.get("active").is_some());
            assert!(parsed.get("committed").is_some());
            assert!(parsed.get("active_statements").is_some());

            assert!(linkrs_transaction_list(ptr::null_mut()).is_null());
            assert!(linkrs_transaction_metrics(ptr::null_mut()).is_null());

            assert_eq!(linkrs_close(db), 0);
        }
    }

    #[test]
    fn test_transaction_list_active_transaction() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_free_string};
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};

        unsafe {
            let db = open_memory_db();

            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut txn: *mut linkrs_txn_t = ptr::null_mut();
            assert_eq!(
                linkrs_txn_begin(session, &mut txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let list = linkrs_transaction_list(db);
            assert!(!list.is_null());
            let json = CStr::from_ptr(list).to_str().expect("utf8").to_string();
            linkrs_free_string(list);
            let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).expect("list json");
            assert_eq!(parsed.len(), 1);
            assert!(parsed[0].get("transaction_id").is_some());

            assert_eq!(
                linkrs_txn_rollback(txn),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            linkrs_txn_free(txn);
            linkrs_session_close(session);
            assert_eq!(linkrs_close(db), 0);
        }
    }
}

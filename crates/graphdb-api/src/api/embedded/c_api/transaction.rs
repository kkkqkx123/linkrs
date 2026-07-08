//! C API Transaction Management Module
//!
//! Provides transaction management functionality, including transaction start, commit, rollback, and savepoints

use crate::api::core::TransactionHandle;
use crate::api::embedded::c_api::error::{
    error_code_from_core_error, graphdb_error_code_t, set_last_error_message,
};
use crate::api::embedded::c_api::result::GraphDbResultHandle;
use crate::api::embedded::c_api::session::GraphDbSessionHandle;
use crate::api::embedded::c_api::types::{graphdb_result_t, graphdb_session_t, graphdb_txn_t};
use std::ffi::{c_char, c_int, CStr};
use std::ptr;

/// Internal structure of transaction handles
///
/// Note: This structure holds the session pointer, but does not own the session.
/// The caller must ensure that the session is not closed until the transaction completes.
pub struct GraphDbTxnHandle {
    pub(crate) session: *mut GraphDbSessionHandle,
    pub(crate) txn_handle: Option<TransactionHandle>,
    pub(crate) committed: bool,
    pub(crate) rolled_back: bool,
}

impl GraphDbTxnHandle {
    /// Check if the session is still active
    fn is_session_valid(&self) -> bool {
        !self.session.is_null()
    }

    /// Get session reference (if valid)
    fn get_session(&self) -> Option<&GraphDbSessionHandle> {
        if self.is_session_valid() {
            Some(unsafe { &*self.session })
        } else {
            None
        }
    }
}

impl Drop for GraphDbTxnHandle {
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
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `txn` must be a valid pointer to store the transaction handle
/// - The session must not have been closed
/// - The caller is responsible for freeing the transaction using `graphdb_txn_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_begin(
    session: *mut graphdb_session_t,
    txn: *mut *mut graphdb_txn_t,
) -> c_int {
    if session.is_null() || txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);

    // Use embedded session API instead of direct TransactionManager access
    match handle.inner.begin_transaction() {
        Ok(transaction) => {
            let txn_handle = transaction.txn_handle();
            let txn_handle_box = Box::new(GraphDbTxnHandle {
                session: session as *mut GraphDbSessionHandle,
                txn_handle: Some(txn_handle),
                committed: false,
                rolled_back: false,
            });
            // Leak the transaction to prevent Drop from being called
            // The transaction lifecycle is now managed by GraphDbTxnHandle
            std::mem::forget(transaction);
            *txn = Box::into_raw(txn_handle_box) as *mut graphdb_txn_t;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let error_code = graphdb_error_code_t::GRAPHDB_ABORT as c_int;
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `txn` must be a valid pointer to store the transaction handle
/// - The session must not have been closed
/// - The caller is responsible for freeing the transaction using `graphdb_txn_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_begin_readonly(
    session: *mut graphdb_session_t,
    txn: *mut *mut graphdb_txn_t,
) -> c_int {
    if session.is_null() || txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);

    // Use embedded session API with read-only config
    let config = crate::api::embedded::TransactionConfig::new().read_only();
    match handle.inner.begin_transaction_with_config(config) {
        Ok(transaction) => {
            let txn_handle = transaction.txn_handle();
            let txn_handle_box = Box::new(GraphDbTxnHandle {
                session: session as *mut GraphDbSessionHandle,
                txn_handle: Some(txn_handle),
                committed: false,
                rolled_back: false,
            });
            std::mem::forget(transaction);
            *txn = Box::into_raw(txn_handle_box) as *mut graphdb_txn_t;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let error_code = graphdb_error_code_t::GRAPHDB_ABORT as c_int;
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - `query` must be a valid pointer to a null-terminated UTF-8 string
/// - `result` must be a valid pointer to store the result handle
/// - The transaction must not have been committed or rolled back
/// - The caller is responsible for freeing the result using `graphdb_result_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_execute(
    txn: *mut graphdb_txn_t,
    query: *const c_char,
    result: *mut *mut graphdb_result_t,
) -> c_int {
    if txn.is_null() || query.is_null() || result.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let query_str = match CStr::from_ptr(query).to_str() {
        Ok(s) => s,
        Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Checking session validity
    let session = match handle.get_session() {
        Some(s) => s,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => return graphdb_error_code_t::GRAPHDB_INTERNAL as c_int,
    };

    let ctx = crate::api::core::QueryRequest {
        space_id: session.inner.space_id(),
        space_name: session.inner.space_name().map(|s| s.to_string()),
        auto_commit: false,
        transaction_id: Some(txn_handle.0),
        parameters: None,
    };

    let mut query_api = session.inner.query_api_mut();
    match query_api.execute(query_str, ctx) {
        Ok(core_result) => {
            let query_result = crate::api::embedded::result::QueryResult::from_core(core_result);
            let result_handle = Box::new(GraphDbResultHandle {
                inner: query_result,
            });
            *result = Box::into_raw(result_handle) as *mut graphdb_result_t;
            graphdb_error_code_t::GRAPHDB_OK as c_int
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

/// Commit transactions
///
/// # Parameters
/// - `txn`: Transaction handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - The transaction must not have been committed or rolled back already
/// - The associated session must still be valid
/// - After calling this function, the transaction handle should be freed using `graphdb_txn_free`
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_commit(txn: *mut graphdb_txn_t) -> c_int {
    if txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Check session validity first (only check pointer, don't borrow)
    if !handle.is_session_valid() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Get session pointer for later use
    let session_ptr = handle.session;

    // Execute commit hook if present
    {
        let session = &*session_ptr;
        if let Some(callback) = session.commit_hook {
            let result = callback(session.commit_hook_user_data);
            if result != 0 {
                return graphdb_txn_rollback(txn);
            }
        }
    }

    let txn_handle = match handle.txn_handle.take() {
        Some(h) => h,
        None => return graphdb_error_code_t::GRAPHDB_INTERNAL as c_int,
    };

    // Use embedded session API instead of direct TransactionManager access
    let session = &*session_ptr;

    // Commit transaction (synchronous, no longer async)
    let result = session.inner.commit_transaction(txn_handle);

    match result {
        Ok(_) => {
            handle.committed = true;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let error_code = graphdb_error_code_t::GRAPHDB_ABORT as c_int;
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - The transaction must not have been committed or rolled back already
/// - The associated session must still be valid
/// - After calling this function, the transaction handle should be freed using `graphdb_txn_free`
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_rollback(txn: *mut graphdb_txn_t) -> c_int {
    if txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Check session validity first (only check pointer, don't borrow)
    if !handle.is_session_valid() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Get session pointer for later use
    let session_ptr = handle.session;

    // Execute rollback hook if present
    {
        let session = &*session_ptr;
        if let Some(callback) = session.rollback_hook {
            callback(session.rollback_hook_user_data);
        }
    }

    let txn_handle = match handle.txn_handle.take() {
        Some(h) => h,
        None => return graphdb_error_code_t::GRAPHDB_INTERNAL as c_int,
    };

    // Use embedded session API instead of direct TransactionManager access
    let session = &*session_ptr;
    match session.inner.rollback_transaction(txn_handle) {
        Ok(_) => {
            handle.rolled_back = true;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        Err(e) => {
            let error_code = graphdb_error_code_t::GRAPHDB_ABORT as c_int;
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
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - `name` must be a valid pointer to a null-terminated UTF-8 string
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_savepoint(
    txn: *mut graphdb_txn_t,
    name: *const c_char,
) -> i64 {
    if txn.is_null() || name.is_null() {
        return -1;
    }

    let name_str = match CStr::from_ptr(name).to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return -1;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => return -1,
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => return -1,
    };

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.create_savepoint(txn_handle, name_str) {
        Ok(id) => id.0 as i64,
        Err(_) => -1,
    }
}

/// Release the savepoint.
///
/// # Parameters
/// - `txn`: Transaction handle
/// - `savepoint_id`: ID of the savepoint
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - `savepoint_id` must be a valid savepoint ID returned by `graphdb_txn_savepoint`
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_release_savepoint(
    txn: *mut graphdb_txn_t,
    savepoint_id: i64,
) -> c_int {
    if txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let savepoint = crate::api::core::SavepointId(savepoint_id as u64);

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.release_savepoint(txn_handle, savepoint) {
        Ok(_) => graphdb_error_code_t::GRAPHDB_OK as c_int,
        Err(e) => {
            let core_error = crate::api::core::CoreError::TransactionFailed(format!("{}", e));
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
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - `savepoint_id` must be a valid savepoint ID returned by `graphdb_txn_savepoint`
/// - The transaction must not have been committed or rolled back
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_rollback_to_savepoint(
    txn: *mut graphdb_txn_t,
    savepoint_id: i64,
) -> c_int {
    if txn.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(txn as *mut GraphDbTxnHandle);

    if handle.committed || handle.rolled_back {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let session = match handle.get_session() {
        Some(s) => s,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let txn_handle = match handle.txn_handle.as_ref() {
        Some(h) => h,
        None => return graphdb_error_code_t::GRAPHDB_INTERNAL as c_int,
    };

    let savepoint = crate::api::core::SavepointId(savepoint_id as u64);

    // Use embedded session API instead of direct TransactionManager access
    match session.inner.rollback_to_savepoint(txn_handle, savepoint) {
        Ok(_) => graphdb_error_code_t::GRAPHDB_OK as c_int,
        Err(e) => {
            let core_error = crate::api::core::CoreError::TransactionFailed(format!("{}", e));
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
/// - `txn` must be a valid transaction handle created by `graphdb_txn_begin` or `graphdb_txn_begin_readonly`
/// - `txn` can be null (in which case this function does nothing)
/// - After calling this function, the handle is invalid and must not be used again
#[no_mangle]
pub unsafe extern "C" fn graphdb_txn_free(txn: *mut graphdb_txn_t) {
    if !txn.is_null() {
        let _ = Box::from_raw(txn as *mut GraphDbTxnHandle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_txn_begin_null_params() {
        let result = unsafe { graphdb_txn_begin(ptr::null_mut(), ptr::null_mut()) };
        assert_eq!(result, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }

    #[test]
    fn test_txn_free_null() {
        // Should not panic
        unsafe { graphdb_txn_free(ptr::null_mut()) };
    }

    #[test]
    fn test_txn_commit_null() {
        let result = unsafe { graphdb_txn_commit(ptr::null_mut()) };
        assert_eq!(result, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }

    #[test]
    fn test_txn_rollback_null() {
        let result = unsafe { graphdb_txn_rollback(ptr::null_mut()) };
        assert_eq!(result, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }
}

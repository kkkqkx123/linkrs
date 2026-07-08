//! C API Statistics Module
//!
//! Provides functions for querying session and query statistics

use crate::api::embedded::c_api::error::graphdb_error_code_t;
use crate::api::embedded::c_api::session::GraphDbSessionHandle;
use crate::api::embedded::c_api::types::graphdb_session_t;
use std::ffi::c_int;

/// Get the number of rows affected by the last operation
///
/// # Arguments
/// - `session`: Session handle
///
/// # Returns
/// - Number of rows affected, returns 0 on error
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
#[no_mangle]
pub unsafe extern "C" fn graphdb_session_changes(session: *mut graphdb_session_t) -> u64 {
    if session.is_null() {
        return 0;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);
    handle.inner.changes()
}

/// Get the total number of rows affected
///
/// # Arguments
/// - `session`: Session handle
///
/// # Returns
/// - Total number of rows affected, returns 0 on error
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
#[no_mangle]
pub unsafe extern "C" fn graphdb_session_total_changes(session: *mut graphdb_session_t) -> u64 {
    if session.is_null() {
        return 0;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);
    handle.inner.total_changes()
}

/// Get the ID of the last inserted vertex
///
/// # Arguments
/// - `session`: Session handle
/// - `vertex_id`: Output parameter, vertex ID
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code (GRAPHDB_NOTFOUND if no vertex was inserted)
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `vertex_id` must be a valid pointer to store the result
#[no_mangle]
pub unsafe extern "C" fn graphdb_session_last_insert_vertex_id(
    session: *mut graphdb_session_t,
    vertex_id: *mut i64,
) -> c_int {
    if session.is_null() || vertex_id.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);
    match handle.inner.last_insert_vertex_id() {
        Some(id) => {
            *vertex_id = id as i64;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Get the ID of the last inserted edge
///
/// # Arguments
/// - `session`: Session handle
/// - `edge_id`: Output parameter, edge ID
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code (GRAPHDB_NOTFOUND if no edge was inserted)
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `edge_id` must be a valid pointer to store the result
#[no_mangle]
pub unsafe extern "C" fn graphdb_session_last_insert_edge_id(
    session: *mut graphdb_session_t,
    edge_id: *mut u64,
) -> c_int {
    if session.is_null() || edge_id.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);
    match handle.inner.last_insert_edge_id() {
        Some(id) => {
            *edge_id = id;
            graphdb_error_code_t::GRAPHDB_OK as c_int
        }
        None => graphdb_error_code_t::GRAPHDB_NOTFOUND as c_int,
    }
}

/// Get session statistics (total changes)
///
/// # Arguments
/// - `session`: Session handle
/// - `stats`: Output parameter, statistics structure
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
#[no_mangle]
pub unsafe extern "C" fn graphdb_session_get_statistics(
    session: *mut graphdb_session_t,
    stats: *mut SessionStatistics,
) -> c_int {
    if session.is_null() || stats.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(session as *mut GraphDbSessionHandle);
    let session_stats = handle.inner.statistics();

    *stats = SessionStatistics {
        last_changes: session_stats.last_changes(),
        total_changes: session_stats.total_changes(),
        last_insert_vertex_id: handle.inner.last_insert_vertex_id().unwrap_or(0),
        last_insert_edge_id: handle.inner.last_insert_edge_id().unwrap_or(0),
    };

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Session statistics structure
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SessionStatistics {
    /// Number of rows affected by the last operation
    pub last_changes: u64,
    /// Total number of rows affected
    pub total_changes: u64,
    /// ID of the last inserted vertex (0 if none)
    pub last_insert_vertex_id: u64,
    /// ID of the last inserted edge (0 if none)
    pub last_insert_edge_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::embedded::GraphDatabase;

    #[test]
    fn test_session_statistics_initial() {
        unsafe {
            let db = GraphDatabase::open_in_memory().unwrap();
            let session = db.session().unwrap();
            let handle = Box::new(GraphDbSessionHandle::new(session));
            let session_ptr = Box::into_raw(handle) as *mut graphdb_session_t;

            assert_eq!(graphdb_session_changes(session_ptr), 0);
            assert_eq!(graphdb_session_total_changes(session_ptr), 0);

            let mut stats = SessionStatistics {
                last_changes: 0,
                total_changes: 0,
                last_insert_vertex_id: 0,
                last_insert_edge_id: 0,
            };
            assert_eq!(graphdb_session_get_statistics(session_ptr, &mut stats), 0);
            assert_eq!(stats.last_changes, 0);
            assert_eq!(stats.total_changes, 0);

            let _ = Box::from_raw(session_ptr as *mut GraphDbSessionHandle);
        }
    }
}

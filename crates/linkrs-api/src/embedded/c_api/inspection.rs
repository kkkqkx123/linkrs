//! C API inspection helpers.
//!
//! Read-only projections of process observability (health and metrics),
//! schema version history, and transaction state. They mirror the JSON
//! payloads returned by the HTTP and gRPC transports so an embedded C
//! consumer sees the same shapes.

use crate::embedded::c_api::database::LinkrsHandle;
use crate::embedded::c_api::error::set_last_error_message;
use crate::embedded::c_api::types::linkrs_t;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

fn into_c_string(json: String) -> *mut c_char {
    CString::new(json)
        .map(|value| value.into_raw())
        .unwrap_or(ptr::null_mut())
}

/// Return the process-internal health payload as JSON.
///
/// Mirrors the `/v1/health` response; listener readiness is reported as
/// `not_applicable` because no network endpoint is involved.
///
/// # Returns
/// Null-terminated JSON object, or null when `db` is null.
///
/// # Memory Management
/// The returned string must be freed with `linkrs_free_string`.
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_health_json(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    let handle = unsafe { &*(db as *mut LinkrsHandle) };
    into_c_string(handle.inner.health().to_string())
}

/// Return the Prometheus metrics of the database as text.
///
/// Byte-identical to the HTTP `/v1/metrics` endpoint and the gRPC
/// `GetMetrics` response for the same database.
///
/// # Returns
/// Null-terminated metrics text, or null when `db` is null.
///
/// # Memory Management
/// The returned string must be freed with `linkrs_free_string`.
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_metrics_text(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    let handle = unsafe { &*(db as *mut LinkrsHandle) };
    into_c_string(handle.inner.metrics_text())
}

/// Return the version history of a vertex tag or edge type as JSON.
///
/// The payload matches `GET /v1/schema/versions/{space}/{label}`. Storage
/// reports unknown labels as errors, which surface through `linkrs_errmsg`;
/// a stored-but-empty history serializes as the literal JSON `null`.
///
/// # Arguments
/// - `db`: Database handle
/// - `space`: Null-terminated space name
/// - `label`: Null-terminated tag or edge type name
/// - `is_edge`: Non-zero selects an edge type, zero a vertex tag
///
/// # Returns
/// Null-terminated JSON string, or null on invalid arguments or error
/// (details through `linkrs_errmsg`).
///
/// # Memory Management
/// The returned string must be freed with `linkrs_free_string`.
///
/// # Safety
/// - `db` must be a valid database handle
/// - `space` and `label` must be valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_schema_version_history_json(
    db: *mut linkrs_t,
    space: *const c_char,
    label: *const c_char,
    is_edge: c_int,
) -> *mut c_char {
    if db.is_null() || space.is_null() || label.is_null() {
        set_last_error_message("invalid argument: null pointer".to_string());
        return ptr::null_mut();
    }

    let space_str = match CStr::from_ptr(space).to_str() {
        Ok(value) => value,
        Err(_) => {
            set_last_error_message("invalid space: not utf8".to_string());
            return ptr::null_mut();
        }
    };
    let label_str = match CStr::from_ptr(label).to_str() {
        Ok(value) => value,
        Err(_) => {
            set_last_error_message("invalid label: not utf8".to_string());
            return ptr::null_mut();
        }
    };

    let handle = unsafe { &*(db as *mut LinkrsHandle) };
    let history = if is_edge != 0 {
        handle.inner.get_edge_version_history(space_str, label_str)
    } else {
        handle
            .inner
            .get_vertex_version_history(space_str, label_str)
    };

    match history {
        Ok(Some(history)) => {
            let json = history.summary_json(space_str, is_edge != 0);
            into_c_string(json.to_string())
        }
        Ok(None) => into_c_string("null".to_string()),
        Err(e) => {
            set_last_error_message(format!("failed to read version history: {e}"));
            ptr::null_mut()
        }
    }
}

/// Return the active transaction list as a JSON array.
///
/// Each entry mirrors the fields of `GET /v1/transactions`.
///
/// # Returns
/// Null-terminated JSON array, or null when `db` is null.
///
/// # Memory Management
/// The returned string must be freed with `linkrs_free_string`.
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_transactions_json(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    let handle = unsafe { &*(db as *mut LinkrsHandle) };
    let transactions: Vec<serde_json::Value> = handle
        .inner
        .list_transactions()
        .into_iter()
        .map(|info| {
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
        })
        .collect();

    into_c_string(serde_json::Value::Array(transactions).to_string())
}

/// Return transaction outcome and resource gauges as JSON.
///
/// Mirrors `GET /v1/transactions/metrics`.
///
/// # Returns
/// Null-terminated JSON object, or null when `db` is null.
///
/// # Memory Management
/// The returned string must be freed with `linkrs_free_string`.
///
/// # Safety
/// - `db` must be a valid database handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_transaction_metrics_json(db: *mut linkrs_t) -> *mut c_char {
    if db.is_null() {
        return ptr::null_mut();
    }
    let handle = unsafe { &*(db as *mut LinkrsHandle) };
    match serde_json::to_string(&handle.inner.transaction_metrics()) {
        Ok(json) => into_c_string(json),
        Err(e) => {
            set_last_error_message(format!("failed to serialize transaction metrics: {e}"));
            ptr::null_mut()
        }
    }
}

use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

use crate::embedded::c_api::database::GraphDbHandle;
use crate::embedded::c_api::error::set_last_error_message;
use crate::embedded::c_api::types::graphdb_t;

/// C representation of a migration report.
#[repr(C)]
pub struct graphdb_migration_report_t {
    pub success: u8,
    pub steps_completed: u64,
    pub rows_migrated: u64,
    /// Null-terminated JSON string of errors array; caller must free via `graphdb_free_string`.
    pub errors_json: *mut c_char,
}

/// Execute a migration plan given as JSON.
///
/// # Arguments
/// - `db`: Database handle
/// - `plan_json`: Null-terminated JSON string of `MigrationPlan`
///
/// # Returns
/// - On success: pointer to `graphdb_migration_report_t` (must be freed with `graphdb_migration_report_free`)
/// - On failure: null pointer (error details via `graphdb_errmsg`)
///
/// # Safety
/// - `db` must be a valid handle from `graphdb_open`
/// - `plan_json` must be a valid null-terminated UTF-8 string
/// - Returned pointer must be freed by caller
#[no_mangle]
pub unsafe extern "C" fn graphdb_migration_execute(
    db: *mut graphdb_t,
    plan_json: *const c_char,
) -> *mut graphdb_migration_report_t {
    if db.is_null() || plan_json.is_null() {
        set_last_error_message("invalid argument: null pointer".to_string());
        return ptr::null_mut();
    }

    let plan_str = unsafe {
        match CStr::from_ptr(plan_json).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error_message("invalid plan_json: not utf8".to_string());
                return ptr::null_mut();
            }
        }
    };

    let plan: graphdb_migration::MigrationPlan = match serde_json::from_str(plan_str) {
        Ok(p) => p,
        Err(e) => {
            set_last_error_message(format!("failed to parse plan json: {}", e));
            return ptr::null_mut();
        }
    };

    let handle = unsafe { &*(db as *mut GraphDbHandle) };
    let report = match handle.inner.execute_migration_plan(&plan) {
        Ok(r) => r,
        Err(e) => {
            set_last_error_message(format!("migration failed: {}", e));
            return ptr::null_mut();
        }
    };

    let errors_json = serde_json::to_string(&report.errors).unwrap_or_else(|_| "[]".to_string());
    let errors_c = match CString::new(errors_json) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    };

    let c_report = Box::new(graphdb_migration_report_t {
        success: if report.success { 1 } else { 0 },
        steps_completed: report.steps_completed as u64,
        rows_migrated: report.rows_migrated,
        errors_json: errors_c,
    });

    Box::into_raw(c_report)
}

/// Free a migration report returned by `graphdb_migration_execute`.
///
/// # Safety
/// - `report` must be a valid pointer from `graphdb_migration_execute` or null
#[no_mangle]
pub unsafe extern "C" fn graphdb_migration_report_free(report: *mut graphdb_migration_report_t) {
    if report.is_null() {
        return;
    }
    unsafe {
        let r = Box::from_raw(report);
        if !r.errors_json.is_null() {
            let _ = CString::from_raw(r.errors_json);
        }
    }
}

/// Generate a migration plan and return it as JSON string.
///
/// Caller must free the returned string with `graphdb_free_string`.
///
/// # Safety
/// - `db` must be valid
/// - `space`, `label` must be valid null-terminated strings
#[no_mangle]
pub unsafe extern "C" fn graphdb_migration_plan_json(
    db: *mut graphdb_t,
    space: *const c_char,
    label: *const c_char,
    is_edge: c_int,
    from_version: u64,
    to_version: u64,
) -> *mut c_char {
    if db.is_null() || space.is_null() || label.is_null() {
        set_last_error_message("invalid argument: null pointer".to_string());
        return ptr::null_mut();
    }

    let space_str = unsafe {
        match CStr::from_ptr(space).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error_message("invalid space".to_string());
                return ptr::null_mut();
            }
        }
    };
    let label_str = unsafe {
        match CStr::from_ptr(label).to_str() {
            Ok(s) => s,
            Err(_) => {
                set_last_error_message("invalid label".to_string());
                return ptr::null_mut();
            }
        }
    };

    let handle = unsafe { &*(db as *mut GraphDbHandle) };
    let plan_res = if is_edge != 0 {
        handle
            .inner
            .generate_edge_migration_plan(space_str, label_str, from_version, to_version)
    } else {
        handle
            .inner
            .generate_vertex_migration_plan(space_str, label_str, from_version, to_version)
    };

    let plan = match plan_res {
        Ok(p) => p,
        Err(e) => {
            set_last_error_message(format!("failed to generate plan: {}", e));
            return ptr::null_mut();
        }
    };

    let json = match serde_json::to_string(&plan) {
        Ok(j) => j,
        Err(e) => {
            set_last_error_message(format!("failed to serialize plan: {}", e));
            return ptr::null_mut();
        }
    };

    match CString::new(json) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

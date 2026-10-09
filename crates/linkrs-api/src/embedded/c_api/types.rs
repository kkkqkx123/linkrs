//! C API Core Type Definitions
//!
//! Define all data types and constants used in the C API.
//! Value-related types are re-exported from core to avoid circular dependencies.

use std::ffi::{c_char, c_int, c_void};

// Re-export value-related types from core (defined in core/types/c_api.rs)
pub use linkrs_core::types::c_api::{
    linkrs_blob_t, linkrs_string_t, linkrs_value_data_t, linkrs_value_t, linkrs_value_type_t,
};

/// Database handle (opaque pointer)
///
/// The `_dummy` member gives the type a real size in C: an empty struct is
/// not valid C11 and MSVC rejects it (C2016).
#[repr(C)]
pub struct linkrs_t {
    _dummy: u8,
}

/// Session handles (opaque pointers)
#[repr(C)]
pub struct linkrs_session_t {
    _dummy: u8,
}

/// Transaction handles (opaque pointers)
#[repr(C)]
pub struct linkrs_txn_t {
    _dummy: u8,
}

/// Result set handle (opaque pointer)
#[repr(C)]
pub struct linkrs_result_t {
    _dummy: u8,
}

/// Batch operation handles (opaque pointers)
#[repr(C)]
pub struct linkrs_batch_t {
    _dummy: u8,
}

/// Busy handler handle (opaque pointer).
///
/// Created by `linkrs_busy_handler_create`, released with
/// `linkrs_busy_handler_free`.
#[repr(C)]
pub struct linkrs_busy_handler_t {
    _dummy: u8,
}

/// Database configuration handle (opaque pointer).
///
/// The handle owns a `DatabaseConfig` and must be created with
/// `linkrs_config_new/_file/_memory`, tuned with the `linkrs_config_set_*`
/// setters, consumed with `linkrs_open_with_config`, and released with
/// `linkrs_config_free`. It is opaque on purpose: pattern matches the other
/// handle types in this module.
#[repr(C)]
pub struct linkrs_config_t {
    _dummy: u8,
}

/// SQL Trace Callback Types
#[allow(non_camel_case_types)]
pub type linkrs_trace_callback = Option<extern "C" fn(sql: *const c_char, user_data: *mut c_void)>;

/// Hook Callback Types
#[allow(non_camel_case_types)]
pub type linkrs_commit_hook_callback = Option<extern "C" fn(user_data: *mut c_void) -> c_int>;
#[allow(non_camel_case_types)]
pub type linkrs_rollback_hook_callback = Option<extern "C" fn(user_data: *mut c_void)>;
#[allow(non_camel_case_types)]
pub type linkrs_update_hook_callback = Option<
    extern "C" fn(
        user_data: *mut c_void,
        operation: c_int,
        database: *const c_char,
        table: *const c_char,
        rowid: i64,
    ),
>;

/// Hook type constants
pub const LINKRS_HOOK_INSERT: c_int = 1;
pub const LINKRS_HOOK_UPDATE: c_int = 2;
pub const LINKRS_HOOK_DELETE: c_int = 3;

/// Extended Error Code
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum linkrs_extended_error_code_t {
    /// No extension error
    LINKRS_EXTENDED_NONE = 0,

    // Parsing Related (1000-1099)
    LINKRS_ERROR_SYNTAX = 1000,
    LINKRS_ERROR_SEMANTIC = 1001,
    LINKRS_ERROR_UNEXPECTED_TOKEN = 1002,
    LINKRS_ERROR_UNTERMINATED_LITERAL = 1003,

    // Type-related (1100-1199)
    LINKRS_ERROR_TYPE_MISMATCH = 1100,
    LINKRS_ERROR_DIVISION_BY_ZERO = 1101,
    LINKRS_ERROR_OUT_OF_RANGE = 1102,

    // Relevant to constraints (1200-1299)
    LINKRS_ERROR_DUPLICATE_KEY = 1200,
    LINKRS_ERROR_FOREIGN_KEY = 1201,
    LINKRS_ERROR_NOT_NULL = 1202,
    LINKRS_ERROR_UNIQUE = 1203,
    LINKRS_ERROR_CHECK = 1204,

    // Concurrency-related (1300-1399)
    LINKRS_ERROR_CONNECTION_LOST = 1300,
    LINKRS_ERROR_DEADLOCK = 1301,
    LINKRS_ERROR_LOCK_TIMEOUT = 1302,
    LINKRS_ERROR_CONFLICT = 1303,
    LINKRS_ERROR_NOT_OWNER = 1304,
    LINKRS_ERROR_ALREADY_COMPLETED = 1305,
    LINKRS_ERROR_RECOVERY_REQUIRED = 1306,

    // Image-related (1400-1499)
    LINKRS_ERROR_INVALID_VERTEX = 1400,
    LINKRS_ERROR_INVALID_EDGE = 1401,
    LINKRS_ERROR_PATH_NOT_FOUND = 1402,

    // Internal (1500-1599)
    LINKRS_ERROR_INTERNAL = 1500,
}

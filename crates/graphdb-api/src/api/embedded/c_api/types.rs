//! C API Core Type Definitions
//!
//! Define all data types and constants used in the C API.
//! Value-related types are re-exported from core to avoid circular dependencies.

use std::ffi::{c_char, c_int, c_void};

// Re-export value-related types from core (defined in core/types/c_api.rs)
pub use crate::core::types::c_api::{
    graphdb_blob_t, graphdb_string_t, graphdb_value_data_t, graphdb_value_t, graphdb_value_type_t,
};

/// Database handle (opaque pointer)
#[repr(C)]
pub struct graphdb_t;

/// Session handles (opaque pointers)
#[repr(C)]
pub struct graphdb_session_t;

/// Transaction handles (opaque pointers)
#[repr(C)]
pub struct graphdb_txn_t;

/// Result set handle (opaque pointer)
#[repr(C)]
pub struct graphdb_result_t;

/// Batch operation handles (opaque pointers)
#[repr(C)]
pub struct graphdb_batch_t;

/// Database Configuration
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct graphdb_config_t {
    /// Read-only or not
    pub read_only: bool,
    /// If it doesn't exist is it created
    pub create_if_missing: bool,
    /// Cache size (MB)
    pub cache_size_mb: c_int,
    /// Maximum number of open files
    pub max_open_files: c_int,
    /// Whether to enable compression
    pub enable_compression: bool,
}

impl Default for graphdb_config_t {
    fn default() -> Self {
        Self {
            read_only: false,
            create_if_missing: true,
            cache_size_mb: 256,
            max_open_files: 1000,
            enable_compression: true,
        }
    }
}

/// Database open flag
pub const GRAPHDB_OPEN_READONLY: c_int = 0x00000001;
pub const GRAPHDB_OPEN_READWRITE: c_int = 0x00000002;
pub const GRAPHDB_OPEN_CREATE: c_int = 0x00000004;
pub const GRAPHDB_OPEN_NOMUTEX: c_int = 0x00008000;
pub const GRAPHDB_OPEN_FULLMUTEX: c_int = 0x00010000;
pub const GRAPHDB_OPEN_SHAREDCACHE: c_int = 0x00020000;
pub const GRAPHDB_OPEN_PRIVATECACHE: c_int = 0x00040000;

/// SQL Trace Callback Types
#[allow(non_camel_case_types)]
pub type graphdb_trace_callback = Option<extern "C" fn(sql: *const c_char, user_data: *mut c_void)>;

/// Hook Callback Types
#[allow(non_camel_case_types)]
pub type graphdb_commit_hook_callback = Option<extern "C" fn(user_data: *mut c_void) -> c_int>;
#[allow(non_camel_case_types)]
pub type graphdb_rollback_hook_callback = Option<extern "C" fn(user_data: *mut c_void)>;
#[allow(non_camel_case_types)]
pub type graphdb_update_hook_callback = Option<
    extern "C" fn(
        user_data: *mut c_void,
        operation: c_int,
        database: *const c_char,
        table: *const c_char,
        rowid: i64,
    ),
>;

/// Hook type constants
pub const GRAPHDB_HOOK_INSERT: c_int = 1;
pub const GRAPHDB_HOOK_UPDATE: c_int = 2;
pub const GRAPHDB_HOOK_DELETE: c_int = 3;

/// Extended Error Code
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum graphdb_extended_error_code_t {
    /// No extension error
    GRAPHDB_EXTENDED_NONE = 0,

    // Parsing Related (1000-1099)
    GRAPHDB_ERROR_SYNTAX = 1000,
    GRAPHDB_ERROR_SEMANTIC = 1001,
    GRAPHDB_ERROR_UNEXPECTED_TOKEN = 1002,
    GRAPHDB_ERROR_UNTERMINATED_LITERAL = 1003,

    // Type-related (1100-1199)
    GRAPHDB_ERROR_TYPE_MISMATCH = 1100,
    GRAPHDB_ERROR_DIVISION_BY_ZERO = 1101,
    GRAPHDB_ERROR_OUT_OF_RANGE = 1102,

    // Relevant to constraints (1200-1299)
    GRAPHDB_ERROR_DUPLICATE_KEY = 1200,
    GRAPHDB_ERROR_FOREIGN_KEY = 1201,
    GRAPHDB_ERROR_NOT_NULL = 1202,
    GRAPHDB_ERROR_UNIQUE = 1203,
    GRAPHDB_ERROR_CHECK = 1204,

    // Concurrency-related (1300-1399)
    GRAPHDB_ERROR_CONNECTION_LOST = 1300,
    GRAPHDB_ERROR_DEADLOCK = 1301,
    GRAPHDB_ERROR_LOCK_TIMEOUT = 1302,

    // Image-related (1400-1499)
    GRAPHDB_ERROR_INVALID_VERTEX = 1400,
    GRAPHDB_ERROR_INVALID_EDGE = 1401,
    GRAPHDB_ERROR_PATH_NOT_FOUND = 1402,
}

//! C API Configuration Management Module
//!
//! Provides configuration management functions for database opening

use crate::api::embedded::c_api::error::graphdb_error_code_t;
use crate::api::embedded::c_api::types::graphdb_config_t;
use crate::api::embedded::DatabaseConfig;
use std::ffi::{c_char, c_int, CStr};
use std::time::Duration;

/// Internal structure of configuration handles
pub struct GraphDbConfigHandle {
    pub(crate) inner: DatabaseConfig,
}

impl GraphDbConfigHandle {
    pub fn new(inner: DatabaseConfig) -> Self {
        Self { inner }
    }
}

/// Create a new configuration (default configuration)
///
/// # Returns
/// - Configuration handle
///
/// # Memory Management
/// The returned configuration must be freed using `graphdb_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `graphdb_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_new() -> *mut graphdb_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut graphdb_config_t
}

/// Create a file database configuration
///
/// # Arguments
/// - `path`: Database file path (UTF-8 encoded)
///
/// # Returns
/// - Configuration handle
///
/// # Safety
/// - `path` must be a valid pointer to a null-terminated UTF-8 string
/// - The returned configuration must be freed using `graphdb_config_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_file(path: *const c_char) -> *mut graphdb_config_t {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    let config = DatabaseConfig::file(path_str);
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut graphdb_config_t
}

/// Create an in-memory database configuration
///
/// # Returns
/// - Configuration handle
///
/// # Memory Management
/// The returned configuration must be freed using `graphdb_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `graphdb_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_memory() -> *mut graphdb_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut graphdb_config_t
}

/// Free configuration handle
///
/// # Arguments
/// - `config`: Configuration handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle created by graphdb_config_new,
///   graphdb_config_file, or graphdb_config_memory
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_free(config: *mut graphdb_config_t) -> c_int {
    if config.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let _ = Box::from_raw(config as *mut GraphDbConfigHandle);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Set cache size
///
/// # Arguments
/// - `config`: Configuration handle
/// - `size_mb`: Cache size in MB
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_set_cache_size(
    config: *mut graphdb_config_t,
    size_mb: c_int,
) -> c_int {
    if config.is_null() || size_mb <= 0 {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_cache_size(size_mb as usize);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Set timeout
///
/// # Arguments
/// - `config`: Configuration handle
/// - `timeout_ms`: Timeout in milliseconds
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_set_timeout(
    config: *mut graphdb_config_t,
    timeout_ms: c_int,
) -> c_int {
    if config.is_null() || timeout_ms < 0 {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle
        .inner
        .clone()
        .with_timeout(Duration::from_millis(timeout_ms as u64));
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Set read-only mode
///
/// # Arguments
/// - `config`: Configuration handle
/// - `read_only`: Read-only flag
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_set_read_only(
    config: *mut graphdb_config_t,
    read_only: c_int,
) -> c_int {
    if config.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_read_only(read_only != 0);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Set create-if-missing flag
///
/// # Arguments
/// - `config`: Configuration handle
/// - `create`: Create-if-missing flag
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_set_create_if_missing(
    config: *mut graphdb_config_t,
    create: c_int,
) -> c_int {
    if config.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_create_if_missing(create != 0);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Set WAL (Write-Ahead Logging) enabled
///
/// # Arguments
/// - `config`: Configuration handle
/// - `enable`: Enable flag
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_config_set_enable_wal(
    config: *mut graphdb_config_t,
    enable: c_int,
) -> c_int {
    if config.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_wal(enable != 0);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_config_new() {
        unsafe {
            let config = graphdb_config_new();
            assert!(!config.is_null());
            assert_eq!(graphdb_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_file() {
        unsafe {
            let path = CString::new("test.db").unwrap();
            let config = graphdb_config_file(path.as_ptr());
            assert!(!config.is_null());
            assert_eq!(graphdb_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_memory() {
        unsafe {
            let config = graphdb_config_memory();
            assert!(!config.is_null());
            assert_eq!(graphdb_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_cache_size() {
        unsafe {
            let config = graphdb_config_memory();
            assert_eq!(graphdb_config_set_cache_size(config, 128), 0);
            assert_eq!(graphdb_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_timeout() {
        unsafe {
            let config = graphdb_config_memory();
            assert_eq!(graphdb_config_set_timeout(config, 5000), 0);
            assert_eq!(graphdb_config_free(config), 0);
        }
    }
}

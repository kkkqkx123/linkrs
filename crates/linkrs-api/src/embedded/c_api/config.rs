//! C API Configuration Management Module
//!
//! Provides configuration management functions for database opening

use crate::embedded::c_api::error::linkrs_error_code_t;
use crate::embedded::c_api::types::linkrs_config_t;
use crate::embedded::{DatabaseConfig, SyncMode};
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
/// The returned configuration must be freed using `linkrs_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `linkrs_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_new() -> *mut linkrs_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
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
/// - The returned configuration must be freed using `linkrs_config_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_file(path: *const c_char) -> *mut linkrs_config_t {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let path_str = match CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return std::ptr::null_mut(),
    };

    let config = DatabaseConfig::file(path_str);
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
}

/// Create an in-memory database configuration
///
/// # Returns
/// - Configuration handle
///
/// # Memory Management
/// The returned configuration must be freed using `linkrs_config_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `linkrs_config_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_memory() -> *mut linkrs_config_t {
    let config = DatabaseConfig::memory();
    let handle = Box::new(GraphDbConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
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
/// - `config` must be a valid configuration handle created by linkrs_config_new,
///   linkrs_config_file, or linkrs_config_memory
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_free(config: *mut linkrs_config_t) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let _ = Box::from_raw(config as *mut GraphDbConfigHandle);
    linkrs_error_code_t::GRAPHDB_OK as c_int
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
pub unsafe extern "C" fn linkrs_config_set_cache_size(
    config: *mut linkrs_config_t,
    size_mb: c_int,
) -> c_int {
    if config.is_null() || size_mb <= 0 {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_cache_size(size_mb as usize);
    linkrs_error_code_t::GRAPHDB_OK as c_int
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
pub unsafe extern "C" fn linkrs_config_set_timeout(
    config: *mut linkrs_config_t,
    timeout_ms: c_int,
) -> c_int {
    if config.is_null() || timeout_ms < 0 {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle
        .inner
        .clone()
        .with_timeout(Duration::from_millis(timeout_ms as u64));
    linkrs_error_code_t::GRAPHDB_OK as c_int
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
pub unsafe extern "C" fn linkrs_config_set_read_only(
    config: *mut linkrs_config_t,
    read_only: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_read_only(read_only != 0);
    linkrs_error_code_t::GRAPHDB_OK as c_int
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
pub unsafe extern "C" fn linkrs_config_set_create_if_missing(
    config: *mut linkrs_config_t,
    create: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_create_if_missing(create != 0);
    linkrs_error_code_t::GRAPHDB_OK as c_int
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
pub unsafe extern "C" fn linkrs_config_set_enable_wal(
    config: *mut linkrs_config_t,
    enable: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_wal(enable != 0);
    linkrs_error_code_t::GRAPHDB_OK as c_int
}

/// Set the synchronization mode (mirrors `SyncMode`).
///
/// # Arguments
/// - `config`: Configuration handle
/// - `mode`: 0 = Full (every write synced), 1 = Normal (default), 2 = Off
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: GRAPHDB_MISUSE for null handles or out-of-range modes
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_sync_mode(
    config: *mut linkrs_config_t,
    mode: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::GRAPHDB_MISUSE as c_int;
    }
    let sync_mode = match mode {
        0 => SyncMode::Full,
        1 => SyncMode::Normal,
        2 => SyncMode::Off,
        _ => return linkrs_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    let handle = &mut *(config as *mut GraphDbConfigHandle);
    handle.inner = handle.inner.clone().with_sync_mode(sync_mode);
    linkrs_error_code_t::GRAPHDB_OK as c_int
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_config_new() {
        unsafe {
            let config = linkrs_config_new();
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_file() {
        unsafe {
            let path = CString::new("test.db").unwrap();
            let config = linkrs_config_file(path.as_ptr());
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_memory() {
        unsafe {
            let config = linkrs_config_memory();
            assert!(!config.is_null());
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_cache_size() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_cache_size(config, 128), 0);
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_set_timeout() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_timeout(config, 5000), 0);
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_config_sync_mode_values() {
        unsafe {
            let config = linkrs_config_memory();
            assert_eq!(linkrs_config_set_sync_mode(config, 0), 0);
            assert_eq!(linkrs_config_set_sync_mode(config, 1), 0);
            assert_eq!(linkrs_config_set_sync_mode(config, 2), 0);
            assert_eq!(
                linkrs_config_set_sync_mode(config, 7),
                linkrs_error_code_t::GRAPHDB_MISUSE as c_int
            );
            assert_eq!(
                linkrs_config_set_sync_mode(std::ptr::null_mut(), 1),
                linkrs_error_code_t::GRAPHDB_MISUSE as c_int
            );
            assert_eq!(linkrs_config_free(config), 0);
        }
    }

    #[test]
    fn test_open_with_config_memory() {
        use crate::embedded::c_api::database::{linkrs_close, linkrs_open_with_config};
        use crate::embedded::c_api::types::linkrs_t;

        unsafe {
            let config = linkrs_config_memory();
            assert!(!config.is_null());
            let mut db: *mut linkrs_t = std::ptr::null_mut();
            assert_eq!(linkrs_open_with_config(config, &mut db), 0);
            assert!(!db.is_null());
            assert_eq!(linkrs_close(db), 0);
            assert_eq!(linkrs_config_free(config), 0);
            let mut db: *mut linkrs_t = std::ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(std::ptr::null_mut(), &mut db),
                linkrs_error_code_t::GRAPHDB_MISUSE as c_int
            );
        }
    }
}

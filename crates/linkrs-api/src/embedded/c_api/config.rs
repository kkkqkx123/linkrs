//! C API Configuration Management Module
//!
//! Provides configuration management functions for database opening

use crate::embedded::c_api::error::{linkrs_error_code_t, set_last_error_message};
use crate::embedded::c_api::types::linkrs_config_t;
use crate::embedded::{DatabaseConfig, SyncMode};
use std::ffi::{c_char, c_int, CStr};
use std::time::Duration;

/// Internal structure of configuration handles
pub struct LinkrsConfigHandle {
    pub(crate) inner: DatabaseConfig,
}

impl LinkrsConfigHandle {
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
    let handle = Box::new(LinkrsConfigHandle::new(config));
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
    let handle = Box::new(LinkrsConfigHandle::new(config));
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
    let handle = Box::new(LinkrsConfigHandle::new(config));
    Box::into_raw(handle) as *mut linkrs_config_t
}

/// Free configuration handle
///
/// # Arguments
/// - `config`: Configuration handle
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: Error code
///
/// # Safety
/// - `config` must be a valid configuration handle created by linkrs_config_new,
///   linkrs_config_file, or linkrs_config_memory
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_free(config: *mut linkrs_config_t) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let _ = Box::from_raw(config as *mut LinkrsConfigHandle);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set cache size
///
/// # Arguments
/// - `config`: Configuration handle
/// - `size_mb`: Cache size in MB
///
/// # Returns
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_cache_size(size_mb as usize);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set timeout
///
/// # Arguments
/// - `config`: Configuration handle
/// - `timeout_ms`: Timeout in milliseconds
///
/// # Returns
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle
        .inner
        .clone()
        .with_timeout(Duration::from_millis(timeout_ms as u64));
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set read-only mode
///
/// # Arguments
/// - `config`: Configuration handle
/// - `read_only`: Read-only flag
///
/// # Returns
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_read_only(read_only != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set create-if-missing flag
///
/// # Arguments
/// - `config`: Configuration handle
/// - `create`: Create-if-missing flag
///
/// # Returns
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_create_if_missing(create != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set WAL (Write-Ahead Logging) enabled
///
/// # Arguments
/// - `config`: Configuration handle
/// - `enable`: Enable flag
///
/// # Returns
/// - Success: LINKRS_OK
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
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_wal(enable != 0);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Set the synchronization mode (mirrors `SyncMode`).
///
/// # Arguments
/// - `config`: Configuration handle
/// - `mode`: 0 = Full (every write synced), 1 = Normal (default), 2 = Off
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: LINKRS_MISUSE for null handles or out-of-range modes
///
/// # Safety
/// - `config` must be a valid configuration handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_config_set_sync_mode(
    config: *mut linkrs_config_t,
    mode: c_int,
) -> c_int {
    if config.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let sync_mode = match mode {
        0 => SyncMode::Full,
        1 => SyncMode::Normal,
        2 => SyncMode::Off,
        _ => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    handle.inner = handle.inner.clone().with_sync_mode(sync_mode);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Apply several configuration updates in one call.
///
/// `updates_json` is a JSON object whose keys mirror the individual
/// `linkrs_config_set_*` setters:
///
/// - `cache_size_mb` (positive integer)
/// - `timeout_ms` (non-negative integer)
/// - `read_only` (boolean)
/// - `create_if_missing` (boolean)
/// - `enable_wal` (boolean)
/// - `sync_mode` (integer: 0 = Full, 1 = Normal, 2 = Off)
/// - `checkpoint_on_close` (boolean)
/// - `vector_enabled` (boolean)
///
/// The update is atomic: unknown keys or invalid values leave the
/// configuration unchanged.
///
/// # Arguments
/// - `config`: Configuration handle
/// - `updates_json`: Null-terminated JSON object string
///
/// # Returns
/// - Success: LINKRS_OK
/// - Failure: LINKRS_MISUSE for null handles, LINKRS_ERROR for malformed
///   JSON or an unknown key, LINKRS_MISUSE for an invalid value
///
/// # Safety
/// - `config` must be a valid configuration handle
/// - `updates_json` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_bulk_update_config(
    config: *mut linkrs_config_t,
    updates_json: *const c_char,
) -> c_int {
    if config.is_null() || updates_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let json_str = match CStr::from_ptr(updates_json).to_str() {
        Ok(value) => value,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    let object = match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(serde_json::Value::Object(object)) => object,
        Ok(_) => {
            set_last_error_message("config updates must be a JSON object".to_string());
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
        Err(e) => {
            set_last_error_message(format!("invalid config JSON: {e}"));
            return linkrs_error_code_t::LINKRS_ERROR as c_int;
        }
    };

    let handle = &mut *(config as *mut LinkrsConfigHandle);
    let mut updated = handle.inner.clone();

    for (key, value) in object.iter() {
        match key.as_str() {
            "cache_size_mb" => match value.as_u64() {
                Some(size) if size > 0 => updated = updated.with_cache_size(size as usize),
                _ => {
                    set_last_error_message("cache_size_mb must be a positive integer".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "timeout_ms" => match value.as_u64() {
                Some(timeout) => {
                    updated = updated.with_timeout(Duration::from_millis(timeout));
                }
                None => {
                    set_last_error_message("timeout_ms must be a non-negative integer".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "read_only" => match value.as_bool() {
                Some(flag) => updated = updated.with_read_only(flag),
                None => {
                    set_last_error_message("read_only must be a boolean".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "create_if_missing" => match value.as_bool() {
                Some(flag) => updated = updated.with_create_if_missing(flag),
                None => {
                    set_last_error_message("create_if_missing must be a boolean".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "enable_wal" => match value.as_bool() {
                Some(flag) => updated = updated.with_wal(flag),
                None => {
                    set_last_error_message("enable_wal must be a boolean".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "sync_mode" => match value.as_i64() {
                Some(0) => updated = updated.with_sync_mode(SyncMode::Full),
                Some(1) => updated = updated.with_sync_mode(SyncMode::Normal),
                Some(2) => updated = updated.with_sync_mode(SyncMode::Off),
                _ => {
                    set_last_error_message("sync_mode must be 0, 1, or 2".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "checkpoint_on_close" => match value.as_bool() {
                Some(flag) => updated = updated.with_checkpoint_on_close(flag),
                None => {
                    set_last_error_message("checkpoint_on_close must be a boolean".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            "vector_enabled" => match value.as_bool() {
                Some(flag) => updated = updated.with_vector_enabled(flag),
                None => {
                    set_last_error_message("vector_enabled must be a boolean".to_string());
                    return linkrs_error_code_t::LINKRS_MISUSE as c_int;
                }
            },
            other => {
                set_last_error_message(format!("unknown configuration key: {other}"));
                return linkrs_error_code_t::LINKRS_ERROR as c_int;
            }
        }
    }

    handle.inner = updated;
    linkrs_error_code_t::LINKRS_OK as c_int
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
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_config_set_sync_mode(std::ptr::null_mut(), 1),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
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
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
        }
    }
}

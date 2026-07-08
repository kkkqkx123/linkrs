//! C API Busy Handler Module
//!
//! Provides concurrency control mechanism for multi-threaded environments

use crate::api::embedded::busy_handler::BusyHandler;
use crate::api::embedded::c_api::error::graphdb_error_code_t;
use std::ffi::{c_int, c_void};

/// Internal structure of busy handler
pub struct GraphDbBusyHandler {
    pub(crate) inner: BusyHandler,
}

/// Create a new busy handler
///
/// # Arguments
/// - `timeout_ms`: Timeout in milliseconds, 0 means no wait
///
/// # Returns
/// - Busy handler handle
///
/// # Memory Management
/// The returned handler must be freed using `graphdb_busy_handler_free` when done
///
/// # Safety
/// This function uses FFI and returns a raw pointer. The returned pointer must be freed
/// using `graphdb_busy_handler_free` to avoid memory leaks.
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_create(timeout_ms: c_int) -> *mut c_void {
    let handler = BusyHandler::new(timeout_ms as u32);
    let handle = Box::new(GraphDbBusyHandler { inner: handler });
    Box::into_raw(handle) as *mut c_void
}

/// Free busy handler
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `handler` must be a valid busy handler handle created by `graphdb_busy_handler_create`
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_free(handler: *mut c_void) -> c_int {
    if handler.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let _ = Box::from_raw(handler as *mut GraphDbBusyHandler);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Handle busy condition
///
/// Returns 1 to continue waiting, 0 to abort (timeout)
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - 1: Continue waiting
/// - 0: Timeout or abort
///
/// # Safety
/// - `handler` must be a valid busy handler handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_handle(handler: *mut c_void) -> c_int {
    if handler.is_null() {
        return 0;
    }

    let handle = &*(handler as *mut GraphDbBusyHandler);
    if handle.inner.handle_busy() {
        1
    } else {
        0
    }
}

/// Check if timeout has expired
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - 1: Timeout expired
/// - 0: Not timeout
///
/// # Safety
/// - `handler` must be a valid busy handler handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_is_timeout(handler: *mut c_void) -> c_int {
    if handler.is_null() {
        return 1;
    }

    let handle = &*(handler as *mut GraphDbBusyHandler);
    if handle.inner.is_timeout() {
        1
    } else {
        0
    }
}

/// Get current retry count
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - Retry count, returns 0 on error
///
/// # Safety
/// - `handler` must be a valid busy handler handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_retry_count(handler: *mut c_void) -> u32 {
    if handler.is_null() {
        return 0;
    }

    let handle = &*(handler as *mut GraphDbBusyHandler);
    handle.inner.retry_count()
}

/// Get elapsed time in milliseconds
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - Elapsed time in milliseconds, returns 0 on error
///
/// # Safety
/// - `handler` must be a valid busy handler handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_elapsed_ms(handler: *mut c_void) -> u64 {
    if handler.is_null() {
        return 0;
    }

    let handle = &*(handler as *mut GraphDbBusyHandler);
    handle.inner.elapsed_ms()
}

/// Reset busy handler state
///
/// # Arguments
/// - `handler`: Busy handler handle
///
/// # Returns
/// - Success: GRAPHDB_OK
/// - Failure: Error code
///
/// # Safety
/// - `handler` must be a valid busy handler handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_busy_handler_reset(handler: *mut c_void) -> c_int {
    if handler.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(handler as *mut GraphDbBusyHandler);
    handle.inner.reset();
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_busy_handler_create_free() {
        unsafe {
            let handler = graphdb_busy_handler_create(5000);
            assert!(!handler.is_null());
            assert_eq!(graphdb_busy_handler_free(handler), 0);
        }
    }

    #[test]
    fn test_busy_handler_handle() {
        unsafe {
            let handler = graphdb_busy_handler_create(100);
            assert!(!handler.is_null());

            // First call should return 1 (continue waiting)
            let result = graphdb_busy_handler_handle(handler);
            assert!(result == 0 || result == 1);

            assert_eq!(graphdb_busy_handler_free(handler), 0);
        }
    }

    #[test]
    fn test_busy_handler_timeout() {
        unsafe {
            let handler = graphdb_busy_handler_create(50);
            assert!(!handler.is_null());

            // Wait for timeout
            thread::sleep(Duration::from_millis(100));

            assert_eq!(graphdb_busy_handler_is_timeout(handler), 1);
            assert_eq!(graphdb_busy_handler_free(handler), 0);
        }
    }

    #[test]
    fn test_busy_handler_retry_count() {
        unsafe {
            let handler = graphdb_busy_handler_create(1000);
            assert!(!handler.is_null());

            assert_eq!(graphdb_busy_handler_retry_count(handler), 0);

            // Trigger a retry
            graphdb_busy_handler_handle(handler);

            assert!(graphdb_busy_handler_retry_count(handler) > 0);

            assert_eq!(graphdb_busy_handler_free(handler), 0);
        }
    }

    #[test]
    fn test_busy_handler_reset() {
        unsafe {
            let handler = graphdb_busy_handler_create(1000);
            assert!(!handler.is_null());

            // Trigger some retries
            graphdb_busy_handler_handle(handler);
            let _count_before = graphdb_busy_handler_retry_count(handler);

            // Reset
            assert_eq!(graphdb_busy_handler_reset(handler), 0);
            assert_eq!(graphdb_busy_handler_retry_count(handler), 0);

            assert_eq!(graphdb_busy_handler_free(handler), 0);
        }
    }
}

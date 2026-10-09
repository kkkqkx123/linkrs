//! C API error handling
//!
//! Provide error code conversion and error message management functions.

use crate::api_core::{CoreError, ExtendedErrorCode};
use crate::embedded::c_api::types::{linkrs_extended_error_code_t, linkrs_session_t};
use std::cell::RefCell;
use std::ffi::CString;

fn transaction_extended_error_code(message: &str) -> linkrs_extended_error_code_t {
    let message = message.to_ascii_lowercase();
    if message.contains("write_transaction_conflict")
        || message.contains("transaction conflict")
        || message.contains("conflict")
    {
        linkrs_extended_error_code_t::GRAPHDB_ERROR_CONFLICT
    } else if message.contains("transaction_not_owner") || message.contains("not owned by another")
    {
        linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_OWNER
    } else if message.contains("invalid_state_for_commit")
        || message.contains("already committed")
        || message.contains("already aborted")
    {
        linkrs_extended_error_code_t::GRAPHDB_ERROR_ALREADY_COMPLETED
    } else if message.contains("timeout") {
        linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT
    } else {
        linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE
    }
}

thread_local! {
    static LAST_ERROR_MESSAGE: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Set the final error message
pub(crate) fn set_last_error_message(msg: String) {
    LAST_ERROR_MESSAGE.with(|m| {
        *m.borrow_mut() = CString::new(msg).ok();
    });
}

/// The extended error code is inferred from the CoreError.
pub fn extended_error_code_from_core_error(error: &CoreError) -> linkrs_extended_error_code_t {
    match error {
        CoreError::DetailedQueryError { extended_code, .. } => {
            extended_error_code_from_internal(*extended_code)
        }
        CoreError::QueryExecutionFailed(msg) => {
            if msg.contains("syntax") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_SYNTAX
            } else if msg.contains("semantic") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_SEMANTIC
            } else if msg.contains("type mismatch") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_TYPE_MISMATCH
            } else if msg.contains("constraint") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_CHECK
            } else if msg.contains("division by zero") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DIVISION_BY_ZERO
            } else if msg.contains("out of range") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_OUT_OF_RANGE
            } else if msg.contains("duplicate") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DUPLICATE_KEY
            } else if msg.contains("not null") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_NULL
            } else if msg.contains("unique") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_UNIQUE
            } else if msg.contains("foreign key") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_FOREIGN_KEY
            } else if msg.contains("deadlock") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DEADLOCK
            } else if msg.contains("timeout") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT
            } else if msg.contains("connection") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_CONNECTION_LOST
            } else if msg.contains("vertex") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_VERTEX
            } else if msg.contains("edge") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_EDGE
            } else if msg.contains("path") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_PATH_NOT_FOUND
            } else {
                linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE
            }
        }
        CoreError::StorageError(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::TransactionFailed(message) => transaction_extended_error_code(message),
        CoreError::SchemaOperationFailed(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::InvalidParameter(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::NotFound(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::Internal(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::SyncError(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        CoreError::VectorError(_) => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
    }
}

/// Convert the internal ExtendedErrorCode to a C API extended error code.
fn extended_error_code_from_internal(code: ExtendedErrorCode) -> linkrs_extended_error_code_t {
    match code {
        ExtendedErrorCode::None => linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ExtendedErrorCode::SyntaxError => linkrs_extended_error_code_t::GRAPHDB_ERROR_SYNTAX,
        ExtendedErrorCode::SemanticError => linkrs_extended_error_code_t::GRAPHDB_ERROR_SEMANTIC,
        ExtendedErrorCode::UnexpectedToken => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_UNEXPECTED_TOKEN
        }
        ExtendedErrorCode::UnterminatedLiteral => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_UNTERMINATED_LITERAL
        }
        ExtendedErrorCode::TypeMismatch => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_TYPE_MISMATCH
        }
        ExtendedErrorCode::DivisionByZero => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_DIVISION_BY_ZERO
        }
        ExtendedErrorCode::OutOfRange => linkrs_extended_error_code_t::GRAPHDB_ERROR_OUT_OF_RANGE,
        ExtendedErrorCode::DuplicateKey => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_DUPLICATE_KEY
        }
        ExtendedErrorCode::ForeignKeyConstraint => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_FOREIGN_KEY
        }
        ExtendedErrorCode::NotNullConstraint => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_NULL
        }
        ExtendedErrorCode::UniqueConstraint => linkrs_extended_error_code_t::GRAPHDB_ERROR_UNIQUE,
        ExtendedErrorCode::CheckConstraint => linkrs_extended_error_code_t::GRAPHDB_ERROR_CHECK,
        ExtendedErrorCode::ConnectionLost => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_CONNECTION_LOST
        }
        ExtendedErrorCode::Deadlock => linkrs_extended_error_code_t::GRAPHDB_ERROR_DEADLOCK,
        ExtendedErrorCode::LockTimeout => linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT,
        ExtendedErrorCode::InvalidVertex => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_VERTEX
        }
        ExtendedErrorCode::InvalidEdge => linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_EDGE,
        ExtendedErrorCode::PathNotFound => {
            linkrs_extended_error_code_t::GRAPHDB_ERROR_PATH_NOT_FOUND
        }
        ExtendedErrorCode::Internal => linkrs_extended_error_code_t::GRAPHDB_ERROR_INTERNAL,
    }
}

/// Error code
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum linkrs_error_code_t {
    /// Success
    GRAPHDB_OK = 0,
    /// Common errors
    GRAPHDB_ERROR = 1,
    /// Internal error
    GRAPHDB_INTERNAL = 2,
    /// The permission was denied.
    GRAPHDB_PERM = 3,
    /// The operation was terminated.
    GRAPHDB_ABORT = 4,
    /// The database is busy.
    GRAPHDB_BUSY = 5,
    /// The database is locked.
    GRAPHDB_LOCKED = 6,
    /// Insufficient memory
    GRAPHDB_NOMEM = 7,
    /// Read-only
    GRAPHDB_READONLY = 8,
    /// The operation was interrupted.
    GRAPHDB_INTERRUPT = 9,
    /// IO error
    GRAPHDB_IOERR = 10,
    /// Data corruption
    GRAPHDB_CORRUPT = 11,
    /// Nothing was found.
    GRAPHDB_NOTFOUND = 12,
    /// The disk is full.
    GRAPHDB_FULL = 13,
    /// Unable to open.
    GRAPHDB_CANTOPEN = 14,
    /// Protocol error
    GRAPHDB_PROTOCOL = 15,
    /// Pattern error
    GRAPHDB_SCHEMA = 16,
    /// The data volume is too large.
    GRAPHDB_TOOBIG = 17,
    /// Violation of constraints
    GRAPHDB_CONSTRAINT = 18,
    /// Type mismatch.
    GRAPHDB_MISMATCH = 19,
    /// Misuse
    GRAPHDB_MISUSE = 20,
    /// Out of range
    GRAPHDB_RANGE = 21,
    /// Not implemented
    GRAPHDB_NOT_IMPLEMENTED = 22,
}

impl TryFrom<i32> for linkrs_error_code_t {
    type Error = ();

    fn try_from(code: i32) -> Result<Self, Self::Error> {
        Ok(match code {
            0 => linkrs_error_code_t::GRAPHDB_OK,
            1 => linkrs_error_code_t::GRAPHDB_ERROR,
            2 => linkrs_error_code_t::GRAPHDB_INTERNAL,
            3 => linkrs_error_code_t::GRAPHDB_PERM,
            4 => linkrs_error_code_t::GRAPHDB_ABORT,
            5 => linkrs_error_code_t::GRAPHDB_BUSY,
            6 => linkrs_error_code_t::GRAPHDB_LOCKED,
            7 => linkrs_error_code_t::GRAPHDB_NOMEM,
            8 => linkrs_error_code_t::GRAPHDB_READONLY,
            9 => linkrs_error_code_t::GRAPHDB_INTERRUPT,
            10 => linkrs_error_code_t::GRAPHDB_IOERR,
            11 => linkrs_error_code_t::GRAPHDB_CORRUPT,
            12 => linkrs_error_code_t::GRAPHDB_NOTFOUND,
            13 => linkrs_error_code_t::GRAPHDB_FULL,
            14 => linkrs_error_code_t::GRAPHDB_CANTOPEN,
            15 => linkrs_error_code_t::GRAPHDB_PROTOCOL,
            16 => linkrs_error_code_t::GRAPHDB_SCHEMA,
            17 => linkrs_error_code_t::GRAPHDB_TOOBIG,
            18 => linkrs_error_code_t::GRAPHDB_CONSTRAINT,
            19 => linkrs_error_code_t::GRAPHDB_MISMATCH,
            20 => linkrs_error_code_t::GRAPHDB_MISUSE,
            21 => linkrs_error_code_t::GRAPHDB_RANGE,
            22 => linkrs_error_code_t::GRAPHDB_NOT_IMPLEMENTED,
            _ => return Err(()),
        })
    }
}

/// Converting from core error codes to C error codes and extended error codes
pub fn error_code_from_core_error(error: &CoreError) -> (i32, linkrs_extended_error_code_t) {
    // Read-only violations surface as GRAPHDB_READONLY regardless of the
    // carrying variant (the session gate emits them as `StorageError`).
    if error.to_string().contains("read-only") {
        return (
            linkrs_error_code_t::GRAPHDB_READONLY as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        );
    }
    match error {
        CoreError::DetailedQueryError { extended_code, .. } => {
            let basic_code = match extended_code {
                ExtendedErrorCode::SyntaxError
                | ExtendedErrorCode::SemanticError
                | ExtendedErrorCode::UnexpectedToken
                | ExtendedErrorCode::UnterminatedLiteral => {
                    linkrs_error_code_t::GRAPHDB_ERROR as i32
                }
                ExtendedErrorCode::TypeMismatch => linkrs_error_code_t::GRAPHDB_MISMATCH as i32,
                ExtendedErrorCode::DivisionByZero => linkrs_error_code_t::GRAPHDB_RANGE as i32,
                ExtendedErrorCode::OutOfRange => linkrs_error_code_t::GRAPHDB_RANGE as i32,
                ExtendedErrorCode::DuplicateKey => linkrs_error_code_t::GRAPHDB_CONSTRAINT as i32,
                ExtendedErrorCode::ForeignKeyConstraint => {
                    linkrs_error_code_t::GRAPHDB_CONSTRAINT as i32
                }
                ExtendedErrorCode::NotNullConstraint => {
                    linkrs_error_code_t::GRAPHDB_CONSTRAINT as i32
                }
                ExtendedErrorCode::UniqueConstraint => {
                    linkrs_error_code_t::GRAPHDB_CONSTRAINT as i32
                }
                ExtendedErrorCode::CheckConstraint => {
                    linkrs_error_code_t::GRAPHDB_CONSTRAINT as i32
                }
                ExtendedErrorCode::ConnectionLost => linkrs_error_code_t::GRAPHDB_IOERR as i32,
                ExtendedErrorCode::Deadlock => linkrs_error_code_t::GRAPHDB_BUSY as i32,
                ExtendedErrorCode::LockTimeout => linkrs_error_code_t::GRAPHDB_BUSY as i32,
                ExtendedErrorCode::InvalidVertex
                | ExtendedErrorCode::InvalidEdge
                | ExtendedErrorCode::PathNotFound => linkrs_error_code_t::GRAPHDB_NOTFOUND as i32,
                ExtendedErrorCode::Internal => linkrs_error_code_t::GRAPHDB_ERROR as i32,
                ExtendedErrorCode::None => linkrs_error_code_t::GRAPHDB_OK as i32,
            };
            (
                basic_code,
                extended_error_code_from_internal(*extended_code),
            )
        }
        CoreError::StorageError(_) => (
            linkrs_error_code_t::GRAPHDB_IOERR as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::QueryExecutionFailed(msg) => {
            let extended_code = if msg.contains("syntax") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_SYNTAX
            } else if msg.contains("semantic") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_SEMANTIC
            } else if msg.contains("type mismatch") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_TYPE_MISMATCH
            } else if msg.contains("constraint") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_CHECK
            } else if msg.contains("division by zero") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DIVISION_BY_ZERO
            } else if msg.contains("out of range") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_OUT_OF_RANGE
            } else if msg.contains("duplicate") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DUPLICATE_KEY
            } else if msg.contains("not null") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_NULL
            } else if msg.contains("unique") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_UNIQUE
            } else if msg.contains("foreign key") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_FOREIGN_KEY
            } else if msg.contains("deadlock") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_DEADLOCK
            } else if msg.contains("timeout") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT
            } else if msg.contains("connection") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_CONNECTION_LOST
            } else if msg.contains("vertex") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_VERTEX
            } else if msg.contains("edge") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_EDGE
            } else if msg.contains("path") {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_PATH_NOT_FOUND
            } else {
                linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE
            };
            (linkrs_error_code_t::GRAPHDB_ERROR as i32, extended_code)
        }
        CoreError::TransactionFailed(message) => {
            let extended_code = transaction_extended_error_code(message);
            let basic_code = match extended_code {
                linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_OWNER => {
                    linkrs_error_code_t::GRAPHDB_PERM
                }
                linkrs_extended_error_code_t::GRAPHDB_ERROR_CONFLICT
                | linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT => {
                    linkrs_error_code_t::GRAPHDB_BUSY
                }
                linkrs_extended_error_code_t::GRAPHDB_ERROR_RECOVERY_REQUIRED => {
                    linkrs_error_code_t::GRAPHDB_CORRUPT
                }
                linkrs_extended_error_code_t::GRAPHDB_ERROR_ALREADY_COMPLETED => {
                    linkrs_error_code_t::GRAPHDB_ABORT
                }
                _ => linkrs_error_code_t::GRAPHDB_ABORT,
            };
            (basic_code as i32, extended_code)
        }
        CoreError::SchemaOperationFailed(_) => (
            linkrs_error_code_t::GRAPHDB_SCHEMA as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::Internal(_) => (
            linkrs_error_code_t::GRAPHDB_INTERNAL as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::NotFound(_) => (
            linkrs_error_code_t::GRAPHDB_NOTFOUND as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::InvalidParameter(_) => (
            linkrs_error_code_t::GRAPHDB_MISUSE as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::SyncError(_) => (
            linkrs_error_code_t::GRAPHDB_IOERR as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
        CoreError::VectorError(_) => (
            linkrs_error_code_t::GRAPHDB_IOERR as i32,
            linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE,
        ),
    }
}

/// Retrieve the description message corresponding to the error code (termination if the value is null).
pub fn error_code_to_message(code: linkrs_error_code_t) -> &'static [u8] {
    match code {
        linkrs_error_code_t::GRAPHDB_OK => "OK\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_ERROR => "General error\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_INTERNAL => "Internal error\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_PERM => "Permission denied\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_ABORT => "Operation aborted\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_BUSY => "Database busy\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_LOCKED => "Database locked\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_NOMEM => "Out of memory\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_READONLY => "Read only\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_INTERRUPT => "Operation interrupted\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_IOERR => "IO error\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_CORRUPT => "Data corruption\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_NOTFOUND => "Not found\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_FULL => "Disk full\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_CANTOPEN => "Cannot open\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_PROTOCOL => "Protocol error\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_SCHEMA => "Schema error\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_TOOBIG => "Data too big\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_CONSTRAINT => "Constraint violation\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_MISMATCH => "Type mismatch\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_MISUSE => "Misuse\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_RANGE => "Out of range\0".as_bytes(),
        linkrs_error_code_t::GRAPHDB_NOT_IMPLEMENTED => "Not implemented\0".as_bytes(),
    }
}

/// Retrieve the description message corresponding to the extended error code (termination occurs if the value is null).
pub fn extended_error_code_to_message(code: linkrs_extended_error_code_t) -> &'static [u8] {
    match code {
        linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE => "No error\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_SYNTAX => "Syntax error\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_SEMANTIC => "Semantic error\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_UNEXPECTED_TOKEN => {
            "Unexpected token\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_UNTERMINATED_LITERAL => {
            "Unterminated literal\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_TYPE_MISMATCH => "Type mismatch\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_DIVISION_BY_ZERO => {
            "Division by zero\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_OUT_OF_RANGE => "Out of range\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_DUPLICATE_KEY => "Duplicate key\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_FOREIGN_KEY => {
            "Foreign key constraint\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_NULL => "Not null constraint\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_UNIQUE => "Unique constraint\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_CHECK => "Check constraint\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_CONNECTION_LOST => {
            "Connection lost\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_DEADLOCK => "Deadlock\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_LOCK_TIMEOUT => "Lock timeout\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_CONFLICT => "Transaction conflict\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_NOT_OWNER => {
            "Transaction owner mismatch\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_ALREADY_COMPLETED => {
            "Transaction already completed\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_RECOVERY_REQUIRED => {
            "Transaction recovery required\0".as_bytes()
        }
        linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_VERTEX => "Invalid vertex\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_INVALID_EDGE => "Invalid edge\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_PATH_NOT_FOUND => "Path not found\0".as_bytes(),
        linkrs_extended_error_code_t::GRAPHDB_ERROR_INTERNAL => "Internal error\0".as_bytes(),
    }
}

/// Retrieve the last error message (thread-safe).
///
/// # Arguments
/// - `msg`: Output buffer
/// - `len`: Buffer length
///
/// # Returns
/// - Number of characters actually written (excluding null terminator)
///
/// # Safety
/// - `msg` must be a valid pointer to a buffer with at least `len` bytes
/// - The buffer must be large enough to hold the error message including null terminator
/// - If the message is longer than `len - 1`, it will be truncated
#[no_mangle]
pub unsafe extern "C" fn linkrs_errmsg(msg: *mut std::ffi::c_char, len: usize) -> i32 {
    if msg.is_null() || len == 0 {
        return 0;
    }

    let message = LAST_ERROR_MESSAGE.with(|m| {
        m.borrow().as_ref().map(|s| s.clone()).unwrap_or_else(|| {
            CString::new("No error message").unwrap_or_else(|_| {
                CString::new("?").expect("Failed to create fallback error message")
            })
        })
    });

    let bytes = message.as_bytes_with_nul();
    let copy_len = std::cmp::min(len - 1, bytes.len() - 1);

    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const std::ffi::c_char, msg, copy_len);
        *msg.add(copy_len) = 0;
    }

    copy_len as i32
}

/// Retrieve the string description corresponding to the error code (similar to sqlite3_errstr in SQLite).
///
/// # Parameter
/// - `code`: Error Code
///
/// # Return
/// Error description string (static lifecycle; no need for release)
#[no_mangle]
pub extern "C" fn linkrs_errstr(code: i32) -> *const std::ffi::c_char {
    match linkrs_error_code_t::try_from(code) {
        Ok(code) => error_code_to_message(code).as_ptr() as *const std::ffi::c_char,
        Err(()) => b"Unknown error code\0".as_ptr() as *const std::ffi::c_char,
    }
}

/// Get the location of the SQL error (in terms of character offset).
///
/// # Parameters
/// - `session`: session handle
///
/// # Returns
/// - Character offset of the error location, if there is no error or invalid session return -1
#[no_mangle]
pub extern "C" fn linkrs_error_offset(session: *mut linkrs_session_t) -> std::ffi::c_int {
    if session.is_null() {
        return -1;
    }

    unsafe {
        let handle = &*(session as *mut crate::embedded::c_api::session::GraphDbSessionHandle);
        handle
            .last_error_offset
            .map(|o| o as std::ffi::c_int)
            .unwrap_or(-1)
    }
}

/// Get Extended Error Code
///
/// # Parameters
/// - `session`: session handle
///
/// # Returns
/// - Extended error code, returns 0 if no error or invalid session (GRAPHDB_EXTENDED_NONE)
#[no_mangle]
pub extern "C" fn linkrs_extended_errcode(session: *mut linkrs_session_t) -> std::ffi::c_int {
    if session.is_null() {
        return linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE as std::ffi::c_int;
    }

    unsafe {
        let handle = &*(session as *mut crate::embedded::c_api::session::GraphDbSessionHandle);
        handle
            .last_extended_error
            .map(|e| e as std::ffi::c_int)
            .unwrap_or(linkrs_extended_error_code_t::GRAPHDB_EXTENDED_NONE as std::ffi::c_int)
    }
}

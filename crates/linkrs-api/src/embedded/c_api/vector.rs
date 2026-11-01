//! C API Secondary Vector Index Module
//!
//! Blocking projections of the embedded vector session API: index creation
//! and removal plus similarity search. Requests and results travel as JSON
//! text so the FFI boundary never sees vector engine layouts; the async
//! coordinator calls are bridged on a short-lived current-thread runtime, so
//! these entry points block like `linkrs_migration_execute`.
//!
//! Availability is a runtime property, not a header property: the symbols
//! are always declared, but without the `vector` feature (or without a
//! configured vector backend) they fail with LINKRS_NOT_IMPLEMENTED or the
//! embedded error instead of performing work.

use crate::embedded::c_api::error::linkrs_error_code_t;
#[cfg(feature = "vector")]
use crate::embedded::c_api::error::{
    error_code_from_core_error, extended_error_code_from_core_error,
};
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::linkrs_session_t;
use std::ffi::{c_char, c_int};
#[cfg(feature = "vector")]
use std::ffi::{CStr, CString};
use std::ptr;

#[cfg(feature = "vector")]
use simvec::DistanceMetric;

/// Run an async coordinator call to completion on a throwaway runtime.
///
/// C callers are synchronous, so each vector entry point parks here. This
/// must never be called from inside an async context.
#[cfg(feature = "vector")]
fn block_current_thread<F>(future: F) -> Result<F::Output, String>
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to create blocking runtime: {e}"))
        .map(|rt| rt.block_on(future))
}

/// Parse a distance metric name (case-insensitive).
///
/// Accepts `cosine`, `euclid`/`euclidean`, `dot` and `manhattan`.
#[cfg(feature = "vector")]
fn parse_distance(raw: &str) -> Option<DistanceMetric> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cosine" => Some(DistanceMetric::Cosine),
        "euclid" | "euclidean" => Some(DistanceMetric::Euclid),
        "dot" => Some(DistanceMetric::Dot),
        "manhattan" => Some(DistanceMetric::Manhattan),
        _ => None,
    }
}

/// Create a vector index over `(tag, field)`.
///
/// `distance` names the metric (`cosine`, `euclid`, `dot`, `manhattan`,
/// case-insensitive). On success `*out_collection` receives the heap
/// collection name unless it is NULL, in which case the name is dropped.
///
/// # Safety
/// - `session` must be a valid session handle with a space selected
/// - `tag` / `field` / `distance` must be valid null-terminated UTF-8 strings
/// - `out_collection` must be NULL or a valid out-pointer freed with
///   `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_vector_create_index(
    session: *mut linkrs_session_t,
    tag: *const c_char,
    field: *const c_char,
    vector_size: u64,
    distance: *const c_char,
    out_collection: *mut *mut c_char,
) -> c_int {
    if session.is_null() || tag.is_null() || field.is_null() || distance.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    #[cfg(not(feature = "vector"))]
    {
        let _ = (tag, field, vector_size, distance);
        if !out_collection.is_null() {
            *out_collection = ptr::null_mut();
        }
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        handle.set_error("vector support is not compiled in".to_string(), None, None);
        return linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int;
    }
    #[cfg(feature = "vector")]
    {
        let (Some(tag_str), Some(field_str), Some(distance_str)) = (
            CStr::from_ptr(tag).to_str().ok(),
            CStr::from_ptr(field).to_str().ok(),
            CStr::from_ptr(distance).to_str().ok(),
        ) else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let Some(metric) = parse_distance(distance_str) else {
            let handle = &mut *(session as *mut LinkrsSessionHandle);
            handle.set_error(
                format!("unknown distance metric: {distance_str}"),
                None,
                None,
            );
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        let future =
            handle
                .inner
                .create_vector_index(tag_str, field_str, vector_size as usize, metric);
        let outcome = match block_current_thread(future) {
            Ok(outcome) => outcome,
            Err(msg) => {
                handle.set_error(msg, None, None);
                return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
            }
        };
        match outcome {
            Ok(collection) => {
                handle.clear_error();
                if !out_collection.is_null() {
                    match CString::new(collection) {
                        Ok(c_collection) => *out_collection = c_collection.into_raw(),
                        Err(_) => *out_collection = ptr::null_mut(),
                    }
                }
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{e}");
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                error_code
            }
        }
    }
}

/// Drop the vector index over `(tag, field)`.
///
/// # Safety
/// - `session` must be a valid session handle with a space selected
/// - `tag` / `field` must be valid null-terminated UTF-8 strings
#[no_mangle]
pub unsafe extern "C" fn linkrs_vector_drop_index(
    session: *mut linkrs_session_t,
    tag: *const c_char,
    field: *const c_char,
) -> c_int {
    if session.is_null() || tag.is_null() || field.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    #[cfg(not(feature = "vector"))]
    {
        let _ = (tag, field);
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        handle.set_error("vector support is not compiled in".to_string(), None, None);
        return linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int;
    }
    #[cfg(feature = "vector")]
    {
        let (Some(tag_str), Some(field_str)) = (
            CStr::from_ptr(tag).to_str().ok(),
            CStr::from_ptr(field).to_str().ok(),
        ) else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        let future = handle.inner.drop_vector_index(tag_str, field_str);
        let outcome = match block_current_thread(future) {
            Ok(outcome) => outcome,
            Err(msg) => {
                handle.set_error(msg, None, None);
                return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
            }
        };
        match outcome {
            Ok(()) => {
                handle.clear_error();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{e}");
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                error_code
            }
        }
    }
}

/// Search the vector index over `(tag, field)`.
///
/// `vector_json` is a JSON array of floats (the query embedding);
/// `threshold` below zero disables the similarity cutoff, otherwise only
/// hits at or above the threshold are returned. `*out_json` receives a heap
/// JSON array of `{"doc_id","score","vector","payload"}` objects.
///
/// # Safety
/// - `session` must be a valid session handle with a space selected
/// - `tag` / `field` / `vector_json` must be valid null-terminated UTF-8 strings
/// - `out_json` must be a valid out-pointer; the result must be freed with
///   `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_vector_search(
    session: *mut linkrs_session_t,
    tag: *const c_char,
    field: *const c_char,
    vector_json: *const c_char,
    limit: u64,
    threshold: f32,
    out_json: *mut *mut c_char,
) -> c_int {
    if session.is_null()
        || tag.is_null()
        || field.is_null()
        || vector_json.is_null()
        || out_json.is_null()
    {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    #[cfg(not(feature = "vector"))]
    {
        let _ = (tag, field, vector_json, limit, threshold);
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        handle.set_error("vector support is not compiled in".to_string(), None, None);
        *out_json = ptr::null_mut();
        return linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int;
    }
    #[cfg(feature = "vector")]
    {
        let (Some(tag_str), Some(field_str), Some(vector_raw)) = (
            CStr::from_ptr(tag).to_str().ok(),
            CStr::from_ptr(field).to_str().ok(),
            CStr::from_ptr(vector_json).to_str().ok(),
        ) else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        let query_vector: Vec<f32> = match serde_json::from_str(vector_raw) {
            Ok(v) => v,
            Err(e) => {
                handle.set_error(format!("invalid vector_json: {e}"), None, None);
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_MISUSE as c_int;
            }
        };
        let future: std::pin::Pin<
            Box<
                dyn std::future::Future<
                    Output = crate::api_core::CoreResult<Vec<crate::api_core::VectorSearchResult>>,
                >,
            >,
        > = if threshold >= 0.0 {
            Box::pin(handle.inner.simvec_with_threshold(
                tag_str,
                field_str,
                query_vector,
                limit as usize,
                threshold,
            ))
        } else {
            Box::pin(
                handle
                    .inner
                    .simvec(tag_str, field_str, query_vector, limit as usize),
            )
        };
        let outcome = match block_current_thread(future) {
            Ok(outcome) => outcome,
            Err(msg) => {
                handle.set_error(msg, None, None);
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
            }
        };
        match outcome {
            Ok(results) => {
                handle.clear_error();
                let items: Vec<serde_json::Value> = results
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "doc_id": r.doc_id.to_string(),
                            "score": r.score,
                            "vector": r.vector,
                            "payload": r.payload,
                        })
                    })
                    .collect();
                match serde_json::to_string(&items) {
                    Ok(json) => match CString::new(json) {
                        Ok(c_json) => {
                            *out_json = c_json.into_raw();
                            linkrs_error_code_t::LINKRS_OK as c_int
                        }
                        Err(_) => {
                            *out_json = ptr::null_mut();
                            linkrs_error_code_t::LINKRS_ERROR as c_int
                        }
                    },
                    Err(e) => {
                        handle.set_error(
                            format!("failed to serialize search results: {e}"),
                            None,
                            None,
                        );
                        *out_json = ptr::null_mut();
                        linkrs_error_code_t::LINKRS_ERROR as c_int
                    }
                }
            }
            Err(e) => {
                let (error_code, _) = error_code_from_core_error(&e);
                let error_msg = format!("{e}");
                let offset = e.error_offset();
                let extended_code = Some(extended_error_code_from_core_error(&e));
                handle.set_error(error_msg, offset, extended_code);
                *out_json = ptr::null_mut();
                error_code
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::c_api::config::linkrs_config_memory;
    use crate::embedded::c_api::database::{linkrs_close, linkrs_open_with_config};
    use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
    use crate::embedded::c_api::types::linkrs_t;

    fn create_test_session() -> (*mut linkrs_t, *mut linkrs_session_t) {
        unsafe {
            let config = linkrs_config_memory();
            let mut db: *mut linkrs_t = ptr::null_mut();
            assert_eq!(
                linkrs_open_with_config(config, &mut db),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let mut session: *mut linkrs_session_t = ptr::null_mut();
            assert_eq!(
                linkrs_session_create(db, &mut session),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            (db, session)
        }
    }

    #[test]
    fn test_vector_null_args() {
        unsafe {
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_vector_create_index(
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    ptr::null(),
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_vector_drop_index(ptr::null_mut(), ptr::null(), ptr::null()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_vector_search(
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    -1.0,
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
        }
    }

    #[cfg(feature = "vector")]
    #[test]
    fn test_vector_rejects_unknown_metric() {
        use std::ffi::CString;
        unsafe {
            let (db, session) = create_test_session();
            let tag = CString::new("t").unwrap();
            let field = CString::new("f").unwrap();
            let metric = CString::new("nope").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_vector_create_index(
                    session,
                    tag.as_ptr(),
                    field.as_ptr(),
                    4,
                    metric.as_ptr(),
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[cfg(feature = "vector")]
    #[test]
    fn test_vector_reports_missing_backend() {
        use std::ffi::CString;
        unsafe {
            // In-memory databases have no vector backend: the call must fail
            // cleanly instead of succeeding or crashing.
            let (db, session) = create_test_session();
            let tag = CString::new("t").unwrap();
            let field = CString::new("f").unwrap();
            let vector = CString::new("[0.1, 0.2]").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            let rc = linkrs_vector_search(
                session,
                tag.as_ptr(),
                field.as_ptr(),
                vector.as_ptr(),
                5,
                -1.0,
                &mut out,
            );
            assert_ne!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert!(out.is_null());
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[cfg(not(feature = "vector"))]
    #[test]
    fn test_vector_reports_not_compiled() {
        use std::ffi::CString;
        unsafe {
            let (db, session) = create_test_session();
            let tag = CString::new("t").unwrap();
            let field = CString::new("f").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_vector_drop_index(session, tag.as_ptr(), field.as_ptr()),
                linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int
            );
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }
}

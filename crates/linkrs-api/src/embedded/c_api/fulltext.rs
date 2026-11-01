//! C API Fulltext Search Module
//!
//! Blocking projections of the embedded fulltext session API plus the fused
//! hybrid search. Like the vector module, requests and results travel as
//! JSON text, async coordinator calls are bridged on a short-lived
//! current-thread runtime, and availability is a runtime property: without
//! the `fulltext` feature (hybrid additionally needs `vector`) the symbols
//! stay declared but fail with LINKRS_NOT_IMPLEMENTED.

use crate::embedded::c_api::error::linkrs_error_code_t;
#[cfg(feature = "fulltext")]
use crate::embedded::c_api::error::{
    error_code_from_core_error, extended_error_code_from_core_error,
};
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::linkrs_session_t;
use std::ffi::{c_char, c_int};
#[cfg(feature = "fulltext")]
use std::ffi::{CStr, CString};
use std::ptr;

#[cfg(all(feature = "fulltext", feature = "vector"))]
use linkrs_query::{FusionStrategy, HybridFusionRequest};

/// Run an async coordinator call to completion on a throwaway runtime.
///
/// C callers are synchronous, so each fulltext entry point parks here. This
/// must never be called from inside an async context.
#[cfg(feature = "fulltext")]
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

/// Search the fulltext index over `(tag, field)` with a plain query string.
///
/// `*out_json` receives a heap JSON array of
/// `{"doc_id","score","highlights","matched_fields"}` objects; `doc_id` uses
/// the engine `Value` serde encoding.
///
/// # Safety
/// - `session` must be a valid session handle with a space selected
/// - `tag` / `field` / `query` must be valid null-terminated UTF-8 strings
/// - `out_json` must be a valid out-pointer; the result must be freed with
///   `linkrs_free_string`
#[no_mangle]
pub unsafe extern "C" fn linkrs_fulltext_search(
    session: *mut linkrs_session_t,
    tag: *const c_char,
    field: *const c_char,
    query: *const c_char,
    limit: u64,
    out_json: *mut *mut c_char,
) -> c_int {
    if session.is_null()
        || tag.is_null()
        || field.is_null()
        || query.is_null()
        || out_json.is_null()
    {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    #[cfg(not(feature = "fulltext"))]
    {
        let _ = (tag, field, query, limit);
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        handle.set_error(
            "fulltext support is not compiled in".to_string(),
            None,
            None,
        );
        *out_json = ptr::null_mut();
        return linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int;
    }
    #[cfg(feature = "fulltext")]
    {
        let (Some(tag_str), Some(field_str), Some(query_str)) = (
            CStr::from_ptr(tag).to_str().ok(),
            CStr::from_ptr(field).to_str().ok(),
            CStr::from_ptr(query).to_str().ok(),
        ) else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        let future = handle
            .inner
            .search_fulltext(tag_str, field_str, query_str, limit as usize);
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
                            "doc_id": serde_json::to_value(&r.doc_id)
                                .unwrap_or(serde_json::Value::Null),
                            "score": r.score,
                            "highlights": r.highlights,
                            "matched_fields": r.matched_fields,
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

/// Fused hybrid fulltext + vector search over `(tag, field)`.
///
/// `strategy` is `rrf` (reciprocal rank fusion, `k` fixed at 60) or
/// `weighted` (blend controlled by `alpha`); other names fail with
/// LINKRS_MISUSE. `yield_fields_json` is a JSON array of field names shared
/// by both branches, `query_text` feeds the fulltext branch and
/// `vector_json` (a JSON array of floats) feeds the vector branch.
/// `*out_json` receives a heap JSON array of fused hits ordered by fused
/// score descending (`{"id","score","highlights","payload","source"}`).
///
/// Both branches must be compiled in and configured; otherwise the call
/// fails with LINKRS_NOT_IMPLEMENTED or the embedded error.
///
/// # Safety
/// - `session` must be a valid session handle with a space selected
/// - all string arguments must be valid null-terminated UTF-8 strings
/// - `out_json` must be a valid out-pointer; the result must be freed with
///   `linkrs_free_string`
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn linkrs_hybrid_search(
    session: *mut linkrs_session_t,
    tag: *const c_char,
    field: *const c_char,
    strategy: *const c_char,
    alpha: f32,
    per_branch_limit: u64,
    fused_limit: u64,
    yield_fields_json: *const c_char,
    query_text: *const c_char,
    vector_json: *const c_char,
    out_json: *mut *mut c_char,
) -> c_int {
    if session.is_null()
        || tag.is_null()
        || field.is_null()
        || strategy.is_null()
        || yield_fields_json.is_null()
        || query_text.is_null()
        || vector_json.is_null()
        || out_json.is_null()
    {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    #[cfg(not(all(feature = "fulltext", feature = "vector")))]
    {
        let _ = (
            tag,
            field,
            strategy,
            alpha,
            per_branch_limit,
            fused_limit,
            yield_fields_json,
            query_text,
            vector_json,
        );
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        handle.set_error(
            "hybrid search needs both fulltext and vector support compiled in".to_string(),
            None,
            None,
        );
        *out_json = ptr::null_mut();
        return linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int;
    }
    #[cfg(all(feature = "fulltext", feature = "vector"))]
    {
        let (
            Some(tag_str),
            Some(field_str),
            Some(strategy_str),
            Some(yield_raw),
            Some(query_str),
            Some(vector_raw),
        ) = (
            CStr::from_ptr(tag).to_str().ok(),
            CStr::from_ptr(field).to_str().ok(),
            CStr::from_ptr(strategy).to_str().ok(),
            CStr::from_ptr(yield_fields_json).to_str().ok(),
            CStr::from_ptr(query_text).to_str().ok(),
            CStr::from_ptr(vector_json).to_str().ok(),
        )
        else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let handle = &mut *(session as *mut LinkrsSessionHandle);
        let fusion = match strategy_str.trim().to_ascii_lowercase().as_str() {
            "rrf" => FusionStrategy::Rrf { k: 60 },
            "weighted" => FusionStrategy::Weighted { alpha },
            _ => {
                handle.set_error(
                    format!("unknown fusion strategy: {strategy_str}"),
                    None,
                    None,
                );
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_MISUSE as c_int;
            }
        };
        let yield_fields: Vec<String> = match serde_json::from_str(yield_raw) {
            Ok(fields) => fields,
            Err(e) => {
                handle.set_error(format!("invalid yield_fields_json: {e}"), None, None);
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_MISUSE as c_int;
            }
        };
        let query_vector: Vec<f32> = match serde_json::from_str(vector_raw) {
            Ok(v) => v,
            Err(e) => {
                handle.set_error(format!("invalid vector_json: {e}"), None, None);
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_MISUSE as c_int;
            }
        };
        let request = HybridFusionRequest::new(
            tag_str,
            field_str,
            fusion,
            per_branch_limit as usize,
            fused_limit as usize,
            yield_fields,
        );
        let future = handle.inner.hybrid_search(request, query_str, query_vector);
        let outcome = match block_current_thread(future) {
            Ok(outcome) => outcome,
            Err(msg) => {
                handle.set_error(msg, None, None);
                *out_json = ptr::null_mut();
                return linkrs_error_code_t::LINKRS_INTERNAL as c_int;
            }
        };
        match outcome {
            Ok(hits) => {
                handle.clear_error();
                let items: Vec<serde_json::Value> = hits
                    .into_iter()
                    .map(|hit| {
                        let payload = hit.payload.map(|map| {
                            map.into_iter()
                                .map(|(k, v)| {
                                    (
                                        k,
                                        serde_json::to_value(&v).unwrap_or(serde_json::Value::Null),
                                    )
                                })
                                .collect::<serde_json::Map<String, serde_json::Value>>()
                        });
                        let source = hit.source.map(|map| {
                            map.into_iter()
                                .map(|(k, v)| {
                                    (
                                        k,
                                        serde_json::to_value(&v).unwrap_or(serde_json::Value::Null),
                                    )
                                })
                                .collect::<serde_json::Map<String, serde_json::Value>>()
                        });
                        serde_json::json!({
                            "id": hit.id,
                            "score": hit.score,
                            "highlights": hit.highlights,
                            "payload": payload,
                            "source": source,
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
                            format!("failed to serialize hybrid hits: {e}"),
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
    fn test_fulltext_null_args() {
        unsafe {
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_fulltext_search(
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_hybrid_search(
                    ptr::null_mut(),
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    0.0,
                    0,
                    0,
                    ptr::null(),
                    ptr::null(),
                    ptr::null(),
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
        }
    }

    #[cfg(feature = "fulltext")]
    #[test]
    fn test_fulltext_reports_missing_backend() {
        use std::ffi::CString;
        unsafe {
            // The default embedded configuration leaves fulltext disabled: the
            // call must fail cleanly instead of succeeding or crashing.
            let (db, session) = create_test_session();
            let tag = CString::new("t").unwrap();
            let field = CString::new("f").unwrap();
            let query = CString::new("hello").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            let rc = linkrs_fulltext_search(
                session,
                tag.as_ptr(),
                field.as_ptr(),
                query.as_ptr(),
                5,
                &mut out,
            );
            assert_ne!(rc, linkrs_error_code_t::LINKRS_OK as c_int);
            assert!(out.is_null());
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[cfg(not(feature = "fulltext"))]
    #[test]
    fn test_fulltext_reports_not_compiled() {
        use std::ffi::CString;
        unsafe {
            let (db, session) = create_test_session();
            let tag = CString::new("t").unwrap();
            let field = CString::new("f").unwrap();
            let query = CString::new("hello").unwrap();
            let mut out: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_fulltext_search(
                    session,
                    tag.as_ptr(),
                    field.as_ptr(),
                    query.as_ptr(),
                    5,
                    &mut out
                ),
                linkrs_error_code_t::LINKRS_NOT_IMPLEMENTED as c_int
            );
            assert!(out.is_null());
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }
}

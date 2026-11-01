//! C API Batch Operation Module
//!
//! Provides batch operation functions, supporting batch insert, batch update, and batch delete

use crate::embedded::c_api::error::{linkrs_error_code_t, set_last_error_message};
use crate::embedded::c_api::session::LinkrsSessionHandle;
use crate::embedded::c_api::types::{linkrs_batch_t, linkrs_session_t, linkrs_value_t};
use linkrs_core::types::VertexId;
use linkrs_core::vertex_edge_path::Tag;
use linkrs_core::{Edge, Vertex};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr, CString};
use std::ptr;

// Batch action item type
enum BatchItem {
    Vertex(Vertex),
    Edge(Edge),
}

/// Internal Structure of Batch Operation Handles
///
/// Note: This structure holds the session pointer, but does not own the session.
/// The caller must ensure that the session is not closed until the batch operation handle is released.
pub struct LinkrsBatchHandle {
    /// Associated session pointer (used to verify session validity)
    session_ptr: *mut LinkrsSessionHandle,
    /// Batch size
    batch_size: usize,
    /// Buffer
    buffer: Vec<BatchItem>,
    /// Number of inserted vertices
    vertices_inserted: usize,
    /// Number of inserted edges
    edges_inserted: usize,
    /// Error messages
    errors: Vec<String>,
}

impl LinkrsBatchHandle {
    /// Buffer one vertex and auto-flush when the batch window fills.
    fn buffer_vertex(&mut self, vid_value: &linkrs_core::Value, tag: &str) -> c_int {
        match value_to_vertex_id(vid_value) {
            Some(vid_id) => {
                let vertex = Vertex::new(vid_id, Tag::new(tag.to_string(), HashMap::new()));
                self.buffer.push(BatchItem::Vertex(vertex));
            }
            None => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
        }
        if self.buffer.len() >= self.batch_size {
            if let Err(e) = self.execute() {
                set_last_error_message(e);
                return linkrs_error_code_t::LINKRS_ERROR as c_int;
            }
        }
        linkrs_error_code_t::LINKRS_OK as c_int
    }

    /// Buffer one edge and auto-flush when the batch window fills.
    fn buffer_edge(
        &mut self,
        src_value: &linkrs_core::Value,
        dst_value: &linkrs_core::Value,
        edge_type: &str,
    ) -> c_int {
        let (Some(src_id), Some(dst_id)) =
            (value_to_vertex_id(src_value), value_to_vertex_id(dst_value))
        else {
            return linkrs_error_code_t::LINKRS_MISUSE as c_int;
        };
        let edge = Edge::new(src_id, dst_id, edge_type.to_string(), 0, HashMap::new());
        self.buffer.push(BatchItem::Edge(edge));
        if self.buffer.len() >= self.batch_size {
            if let Err(e) = self.execute() {
                set_last_error_message(e);
                return linkrs_error_code_t::LINKRS_ERROR as c_int;
            }
        }
        linkrs_error_code_t::LINKRS_OK as c_int
    }

    /// Check if the session is still active
    fn is_session_valid(&self) -> bool {
        !self.session_ptr.is_null()
    }

    /// Get session reference (if valid)
    fn get_session(&self) -> Option<&LinkrsSessionHandle> {
        if self.is_session_valid() {
            Some(unsafe { &*self.session_ptr })
        } else {
            None
        }
    }

    /// Flush vertex buffer - using embedded session API instead of direct storage access
    fn flush_vertices(&mut self) -> Result<(), String> {
        // Separate vertices and edges
        let mut vertices = Vec::new();
        let mut remaining = Vec::new();

        for item in self.buffer.drain(..) {
            match item {
                BatchItem::Vertex(v) => vertices.push(v),
                _ => remaining.push(item),
            }
        }

        // Put the edge back into the buffer
        self.buffer.extend(remaining);

        if vertices.is_empty() {
            return Ok(());
        }

        let vertex_count = vertices.len();

        // Use embedded session API instead of direct storage access
        let result = {
            let session = self
                .get_session()
                .ok_or_else(|| "Session invalid or closed".to_string())?;

            session.inner.batch_insert_vertices(vertices)
        };

        match result {
            Ok(_) => {
                self.vertices_inserted += vertex_count;
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Batch insert vertices failed: {}", e);
                self.errors.push(err_msg.clone());
                Err(err_msg)
            }
        }
    }

    /// Flush edge buffer - using embedded session API instead of direct storage access
    fn flush_edges(&mut self) -> Result<(), String> {
        // Separate edges and vertices
        let mut edges = Vec::new();
        let mut remaining = Vec::new();

        for item in self.buffer.drain(..) {
            match item {
                BatchItem::Edge(e) => edges.push(e),
                _ => remaining.push(item),
            }
        }

        // Put vertices back into the buffer
        self.buffer.extend(remaining);

        if edges.is_empty() {
            return Ok(());
        }

        let edge_count = edges.len();

        // Use embedded session API instead of direct storage access
        let result = {
            let session = self
                .get_session()
                .ok_or_else(|| "Session invalid or closed".to_string())?;

            session.inner.batch_insert_edges(edges)
        };

        match result {
            Ok(_) => {
                self.edges_inserted += edge_count;
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Batch insert edges failed: {}", e);
                self.errors.push(err_msg.clone());
                Err(err_msg)
            }
        }
    }

    /// Performs a batch insert, flushing all buffered data
    fn execute(&mut self) -> Result<(), String> {
        // Flush vertices first.
        self.flush_vertices()?;

        // Flush edges
        self.flush_edges()?;

        Ok(())
    }
}

/// Create a batch inserter
///
/// # Parameters
/// - `session`: session handle
/// - `batch_size`: batch size
/// - `batch`: output parameter, batch operation handle
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `linkrs_session_create`
/// - `batch_size` must be a positive integer (if <= 0, defaults to 100)
/// - `batch` must be a valid pointer to store the batch handle
/// - The created batch handle holds a session pointer but does not own the session
/// - The caller must ensure the session is not closed before the batch handle is freed
/// - The caller is responsible for freeing the batch handle using `linkrs_batch_free` when done
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_inserter_create(
    session: *mut linkrs_session_t,
    batch_size: c_int,
    batch: *mut *mut linkrs_batch_t,
) -> c_int {
    if session.is_null() || batch.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let size = if batch_size <= 0 {
        100
    } else {
        batch_size as usize
    };

    let handle = Box::new(LinkrsBatchHandle {
        session_ptr: session as *mut LinkrsSessionHandle,
        batch_size: size,
        buffer: Vec::new(),
        vertices_inserted: 0,
        edges_inserted: 0,
        errors: Vec::new(),
    });

    *batch = Box::into_raw(handle) as *mut linkrs_batch_t;
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Free batch operation handle
///
/// # Parameters
/// - `batch`: batch operation handle
///
/// # Safety
/// - `batch` must be a valid batch handle created by `linkrs_batch_inserter_create`
/// - `batch` can be null (in which case this function does nothing)
/// - After calling this function, the handle is invalid and must not be used again
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_free(batch: *mut linkrs_batch_t) -> c_int {
    if batch.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let _ = Box::from_raw(batch as *mut LinkrsBatchHandle);
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Add a vertex to the batch
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `vid`: vertex ID
/// - `tags`: single tag name (exactly one label is required)
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `vid` must be a valid pointer to a linkrs_value_t
/// - `tags` must be a non-null single tag name (null, empty, or
///   multi-tag comma-separated values are rejected)
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_add_vertex(
    batch: *mut linkrs_batch_t,
    vid: *const linkrs_value_t,
    tags: *const c_char,
) -> c_int {
    if batch.is_null() || vid.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut LinkrsBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Convert C value to core Value
    let vid_value = unsafe { super::value::linkrs_value_to_core(vid) };

    // Parse the single tag: exactly one non-empty label is required.
    if tags.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    let tags_str = match unsafe { CStr::from_ptr(tags).to_str() } {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let mut tag_names = tags_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let single_tag = match (tag_names.next(), tag_names.next()) {
        (Some(name), None) => name.to_string(),
        _ => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    handle.buffer_vertex(&vid_value, &single_tag)
}

/// Add an edge to the batch
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `src_vid`: source vertex ID
/// - `dst_vid`: destination vertex ID
/// - `edge_type`: edge type
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `src_vid` and `dst_vid` must be valid pointers to linkrs_value_t
/// - `edge_type` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_add_edge(
    batch: *mut linkrs_batch_t,
    src_vid: *const linkrs_value_t,
    dst_vid: *const linkrs_value_t,
    edge_type: *const c_char,
) -> c_int {
    if batch.is_null() || src_vid.is_null() || dst_vid.is_null() || edge_type.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut LinkrsBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    // Convert C values to core Values
    let src_value = unsafe { super::value::linkrs_value_to_core(src_vid) };
    let dst_value = unsafe { super::value::linkrs_value_to_core(dst_vid) };

    // Get edge type
    let edge_type_str = match CStr::from_ptr(edge_type).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    handle.buffer_edge(&src_value, &dst_value, &edge_type_str)
}

/// Execute batch operation (flush all buffered data)
///
/// # Parameters
/// - `batch`: batch operation handle
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_execute(batch: *mut linkrs_batch_t) -> c_int {
    if batch.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut LinkrsBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    if let Err(e) = handle.execute() {
        set_last_error_message(e);
        return linkrs_error_code_t::LINKRS_ERROR as c_int;
    }

    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Get the number of vertices inserted
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `count`: output parameter
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `count` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_vertices_inserted(
    batch: *mut linkrs_batch_t,
    count: *mut c_int,
) -> c_int {
    if batch.is_null() || count.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &*(batch as *mut LinkrsBatchHandle);
    *count = handle.vertices_inserted as c_int;

    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Get the number of edges inserted
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `count`: output parameter
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `count` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_edges_inserted(
    batch: *mut linkrs_batch_t,
    count: *mut c_int,
) -> c_int {
    if batch.is_null() || count.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &*(batch as *mut LinkrsBatchHandle);
    *count = handle.edges_inserted as c_int;

    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Get the number of buffered vertices
///
/// # Safety
/// - `batch` must be a valid batch handle or null
///
/// # Returns
/// - Number of buffered vertices, or -1 if batch is null
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_buffered_vertices(batch: *mut linkrs_batch_t) -> c_int {
    if batch.is_null() {
        return -1;
    }

    let handle = &*(batch as *mut LinkrsBatchHandle);
    handle
        .buffer
        .iter()
        .filter(|item| matches!(item, BatchItem::Vertex(_)))
        .count() as c_int
}

/// Get the number of buffered edges
///
/// # Safety
/// - `batch` must be a valid batch handle or null
///
/// # Returns
/// - Number of buffered edges, or -1 if batch is null
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_buffered_edges(batch: *mut linkrs_batch_t) -> c_int {
    if batch.is_null() {
        return -1;
    }

    let handle = &*(batch as *mut LinkrsBatchHandle);
    handle
        .buffer
        .iter()
        .filter(|item| matches!(item, BatchItem::Edge(_)))
        .count() as c_int
}

/// Add many vertices to the batch in one call.
///
/// Every entry shares one tag (exactly one non-empty label, same rule as
/// `linkrs_batch_add_vertex`) and converts with the same vertex-id rule.
/// Entries buffer in order with the same batch-window auto-flush; the walk
/// stops at the first failing entry and reports it.
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `vids`: Vertex ID array (may be NULL only when `count` is 0)
/// - `count`: Number of vertices
/// - `tags`: Single tag name (exactly one label is required)
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - When `count` is nonzero, `vids` must point to at least `count` valid
///   `linkrs_value_t` elements
/// - `tags` must be a non-null single tag name
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_add_vertices(
    batch: *mut linkrs_batch_t,
    vids: *const linkrs_value_t,
    count: usize,
    tags: *const c_char,
) -> c_int {
    if batch.is_null() || tags.is_null() || (count > 0 && vids.is_null()) {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut LinkrsBatchHandle);
    if !handle.is_session_valid() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let tags_str = match CStr::from_ptr(tags).to_str() {
        Ok(s) => s,
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };
    let mut tag_names = tags_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let single_tag = match (tag_names.next(), tag_names.next()) {
        (Some(name), None) => name.to_string(),
        _ => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    for i in 0..count {
        let vid_value = super::value::linkrs_value_to_core(&*vids.add(i));
        let rc = handle.buffer_vertex(&vid_value, &single_tag);
        if rc != linkrs_error_code_t::LINKRS_OK as c_int {
            return rc;
        }
    }
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Add many edges to the batch in one call.
///
/// `src_vids[i]` and `dst_vids[i]` form the i-th edge; all edges share one
/// edge type. Conversion, ordering and auto-flush mirror
/// `linkrs_batch_add_edge`; the walk stops at the first failing entry.
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `src_vids`: Source vertex ID array (may be NULL only when `count` is 0)
/// - `dst_vids`: Destination vertex ID array (may be NULL only when `count` is 0)
/// - `count`: Number of edges
/// - `edge_type`: Edge type (UTF-8 encoded)
///
/// # Returns
/// Success: LINKRS_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - When `count` is nonzero, `src_vids` and `dst_vids` must each point to
///   at least `count` valid `linkrs_value_t` elements
/// - `edge_type` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_add_edges(
    batch: *mut linkrs_batch_t,
    src_vids: *const linkrs_value_t,
    dst_vids: *const linkrs_value_t,
    count: usize,
    edge_type: *const c_char,
) -> c_int {
    if batch.is_null()
        || edge_type.is_null()
        || (count > 0 && (src_vids.is_null() || dst_vids.is_null()))
    {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut LinkrsBatchHandle);
    if !handle.is_session_valid() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }

    let edge_type_str = match CStr::from_ptr(edge_type).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return linkrs_error_code_t::LINKRS_MISUSE as c_int,
    };

    for i in 0..count {
        let src_value = super::value::linkrs_value_to_core(&*src_vids.add(i));
        let dst_value = super::value::linkrs_value_to_core(&*dst_vids.add(i));
        let rc = handle.buffer_edge(&src_value, &dst_value, &edge_type_str);
        if rc != linkrs_error_code_t::LINKRS_OK as c_int {
            return rc;
        }
    }
    linkrs_error_code_t::LINKRS_OK as c_int
}

/// Summarize batch progress as JSON.
///
/// `linkrs_batch_execute` only reports success or failure, so this query
/// exposes the counters the handle already keeps: inserted vertices and
/// edges, still-buffered vertices and edges, the flush error count and the
/// recorded error messages. The handle stays usable after the query.
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `out_json`: Output parameter, heap JSON object string
///
/// # Returns
/// - Success: LINKRS_OK (`*out_json` holds the JSON object)
/// - Failure: Error code (`*out_json` is NULL)
///
/// # Memory Management
/// The returned string is dynamically allocated and must be freed by the
/// caller using `linkrs_free_string` to avoid memory leaks.
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `out_json` must be a valid pointer to store the result
/// - The returned pointer must be freed by the caller to avoid memory leaks
#[no_mangle]
pub unsafe extern "C" fn linkrs_batch_result_json(
    batch: *mut linkrs_batch_t,
    out_json: *mut *mut c_char,
) -> c_int {
    if batch.is_null() || out_json.is_null() {
        return linkrs_error_code_t::LINKRS_MISUSE as c_int;
    }
    *out_json = ptr::null_mut();

    let handle = &*(batch as *mut LinkrsBatchHandle);
    let buffered_vertices = handle
        .buffer
        .iter()
        .filter(|item| matches!(item, BatchItem::Vertex(_)))
        .count();
    let buffered_edges = handle
        .buffer
        .iter()
        .filter(|item| matches!(item, BatchItem::Edge(_)))
        .count();
    let summary = serde_json::json!({
        "vertices_inserted": handle.vertices_inserted,
        "edges_inserted": handle.edges_inserted,
        "buffered_vertices": buffered_vertices,
        "buffered_edges": buffered_edges,
        "error_count": handle.errors.len(),
        "errors": handle.errors,
    });
    match serde_json::to_string(&summary) {
        Ok(json) => match CString::new(json) {
            Ok(c_json) => {
                *out_json = c_json.into_raw();
                linkrs_error_code_t::LINKRS_OK as c_int
            }
            Err(_) => linkrs_error_code_t::LINKRS_ERROR as c_int,
        },
        Err(e) => {
            set_last_error_message(format!("failed to serialize batch summary: {e}"));
            linkrs_error_code_t::LINKRS_ERROR as c_int
        }
    }
}

fn value_to_vertex_id(value: &linkrs_core::Value) -> Option<VertexId> {
    use linkrs_core::Value;
    match value {
        Value::Int(i) => VertexId::try_from_int64(*i as i64).ok(),
        Value::BigInt(i) => VertexId::try_from_int64(*i).ok(),
        Value::String(s) => VertexId::try_from_string(s.as_str()).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_add_many_and_result_json() {
        use crate::embedded::c_api::config::linkrs_config_memory;
        use crate::embedded::c_api::database::{
            linkrs_close, linkrs_free_string, linkrs_open_with_config,
        };
        use crate::embedded::c_api::session::{linkrs_session_close, linkrs_session_create};
        use crate::embedded::c_api::types::{linkrs_session_t, linkrs_t};
        use std::ffi::{CStr, CString};

        fn int_value(v: i64) -> linkrs_value_t {
            linkrs_value_t {
                type_: crate::embedded::c_api::types::linkrs_value_type_t::LINKRS_INT,
                data: crate::embedded::c_api::types::linkrs_value_data_t { integer: v },
            }
        }

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

            // Large window: nothing auto-flushes, so the test observes buffering.
            let mut batch: *mut linkrs_batch_t = ptr::null_mut();
            assert_eq!(
                linkrs_batch_inserter_create(session, 1000, &mut batch),
                linkrs_error_code_t::LINKRS_OK as c_int
            );

            let tag = CString::new("person").unwrap();
            let vids = [int_value(1), int_value(2), int_value(3)];
            assert_eq!(
                linkrs_batch_add_vertices(batch, vids.as_ptr(), 3, tag.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(linkrs_batch_buffered_vertices(batch), 3);

            let edge_type = CString::new("knows").unwrap();
            let srcs = [int_value(1), int_value(2)];
            let dsts = [int_value(2), int_value(3)];
            assert_eq!(
                linkrs_batch_add_edges(batch, srcs.as_ptr(), dsts.as_ptr(), 2, edge_type.as_ptr()),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            assert_eq!(linkrs_batch_buffered_edges(batch), 2);

            let mut summary: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_batch_result_json(batch, &mut summary),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let text = CStr::from_ptr(summary).to_str().expect("utf8").to_string();
            linkrs_free_string(summary);
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("summary json");
            assert_eq!(
                parsed.get("buffered_vertices").and_then(|v| v.as_u64()),
                Some(3)
            );
            assert_eq!(
                parsed.get("buffered_edges").and_then(|v| v.as_u64()),
                Some(2)
            );
            assert_eq!(
                parsed.get("vertices_inserted").and_then(|v| v.as_u64()),
                Some(0)
            );
            assert_eq!(parsed.get("error_count").and_then(|v| v.as_u64()), Some(0));
            assert_eq!(
                linkrs_batch_result_json(ptr::null_mut(), &mut summary),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            // No space is selected, so flushing deterministically fails and
            // the failure is recorded on the handle summary.
            assert_eq!(
                linkrs_batch_execute(batch),
                linkrs_error_code_t::LINKRS_ERROR as c_int
            );
            let mut summary: *mut c_char = ptr::null_mut();
            assert_eq!(
                linkrs_batch_result_json(batch, &mut summary),
                linkrs_error_code_t::LINKRS_OK as c_int
            );
            let text = CStr::from_ptr(summary).to_str().expect("utf8").to_string();
            linkrs_free_string(summary);
            let parsed: serde_json::Value = serde_json::from_str(&text).expect("summary json");
            assert_eq!(parsed.get("error_count").and_then(|v| v.as_u64()), Some(1));

            // Misuse paths.
            assert_eq!(
                linkrs_batch_add_vertices(batch, ptr::null(), 1, tag.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            assert_eq!(
                linkrs_batch_add_edges(batch, srcs.as_ptr(), ptr::null(), 1, edge_type.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );
            let multi = CString::new("a,b").unwrap();
            assert_eq!(
                linkrs_batch_add_vertices(batch, vids.as_ptr(), 1, multi.as_ptr()),
                linkrs_error_code_t::LINKRS_MISUSE as c_int
            );

            linkrs_batch_free(batch);
            linkrs_session_close(session);
            linkrs_close(db);
        }
    }

    #[test]
    fn test_batch_inserter_create_null_params() {
        let result = unsafe {
            linkrs_batch_inserter_create(std::ptr::null_mut(), 100, std::ptr::null_mut())
        };
        assert_eq!(result, linkrs_error_code_t::LINKRS_MISUSE as c_int);
    }

    #[test]
    fn test_batch_free_null() {
        // Should not panic
        unsafe { linkrs_batch_free(std::ptr::null_mut()) };
    }
}

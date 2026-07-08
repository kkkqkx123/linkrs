//! C API Batch Operation Module
//!
//! Provides batch operation functions, supporting batch insert, batch update, and batch delete

use crate::api::embedded::c_api::error::{graphdb_error_code_t, set_last_error_message};
use crate::api::embedded::c_api::session::GraphDbSessionHandle;
use crate::api::embedded::c_api::types::{graphdb_batch_t, graphdb_session_t, graphdb_value_t};
use crate::core::types::VertexId;
use crate::core::vertex_edge_path::Tag;
use crate::core::{Edge, Vertex};
use std::collections::HashMap;
use std::ffi::{c_char, c_int, CStr};

// Batch action item type
enum BatchItem {
    Vertex(Vertex),
    Edge(Edge),
}

/// Internal Structure of Batch Operation Handles
///
/// Note: This structure holds the session pointer, but does not own the session.
/// The caller must ensure that the session is not closed until the batch operation handle is released.
pub struct GraphDbBatchHandle {
    /// Associated session pointer (used to verify session validity)
    session_ptr: *mut GraphDbSessionHandle,
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

impl GraphDbBatchHandle {
    /// Check if the session is still active
    fn is_session_valid(&self) -> bool {
        !self.session_ptr.is_null()
    }

    /// Get session reference (if valid)
    fn get_session(&self) -> Option<&GraphDbSessionHandle> {
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
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `session` must be a valid session handle created by `graphdb_session_create`
/// - `batch_size` must be a positive integer (if <= 0, defaults to 100)
/// - `batch` must be a valid pointer to store the batch handle
/// - The created batch handle holds a session pointer but does not own the session
/// - The caller must ensure the session is not closed before the batch handle is freed
/// - The caller is responsible for freeing the batch handle using `graphdb_batch_free` when done
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_inserter_create(
    session: *mut graphdb_session_t,
    batch_size: c_int,
    batch: *mut *mut graphdb_batch_t,
) -> c_int {
    if session.is_null() || batch.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let size = if batch_size <= 0 {
        100
    } else {
        batch_size as usize
    };

    let handle = Box::new(GraphDbBatchHandle {
        session_ptr: session as *mut GraphDbSessionHandle,
        batch_size: size,
        buffer: Vec::new(),
        vertices_inserted: 0,
        edges_inserted: 0,
        errors: Vec::new(),
    });

    *batch = Box::into_raw(handle) as *mut graphdb_batch_t;
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Free batch operation handle
///
/// # Parameters
/// - `batch`: batch operation handle
///
/// # Safety
/// - `batch` must be a valid batch handle created by `graphdb_batch_inserter_create`
/// - `batch` can be null (in which case this function does nothing)
/// - After calling this function, the handle is invalid and must not be used again
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_free(batch: *mut graphdb_batch_t) -> c_int {
    if batch.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }
    let _ = Box::from_raw(batch as *mut GraphDbBatchHandle);
    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Add a vertex to the batch
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `vid`: vertex ID
/// - `tags`: tag list (comma-separated string)
///
/// # Returns
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `vid` must be a valid pointer to a graphdb_value_t
/// - `tags` can be null
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_add_vertex(
    batch: *mut graphdb_batch_t,
    vid: *const graphdb_value_t,
    tags: *const c_char,
) -> c_int {
    if batch.is_null() || vid.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut GraphDbBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Convert C value to core Value
    let vid_value = unsafe { super::value::graphdb_value_to_core(vid) };
    let vid_id = match value_to_vertex_id(&vid_value) {
        Some(id) => id,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    // Parse tags
    let tag_list = if tags.is_null() {
        Vec::new()
    } else {
        let tags_str = match unsafe { CStr::from_ptr(tags).to_str() } {
            Ok(s) => s,
            Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
        };
        tags_str
            .split(',')
            .map(|s| Tag::new(s.trim().to_string(), HashMap::new()))
            .filter(|t: &Tag| !t.name.is_empty())
            .collect()
    };

    // Create vertex
    let vertex = Vertex::new(vid_id, tag_list);
    handle.buffer.push(BatchItem::Vertex(vertex));

    // Auto-flush if buffer is full
    if handle.buffer.len() >= handle.batch_size {
        if let Err(e) = handle.execute() {
            set_last_error_message(e);
            return graphdb_error_code_t::GRAPHDB_ERROR as c_int;
        }
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
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
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `src_vid` and `dst_vid` must be valid pointers to graphdb_value_t
/// - `edge_type` must be a valid null-terminated UTF-8 string
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_add_edge(
    batch: *mut graphdb_batch_t,
    src_vid: *const graphdb_value_t,
    dst_vid: *const graphdb_value_t,
    edge_type: *const c_char,
) -> c_int {
    if batch.is_null() || src_vid.is_null() || dst_vid.is_null() || edge_type.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut GraphDbBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    // Convert C values to core Values
    let src_value = unsafe { super::value::graphdb_value_to_core(src_vid) };
    let dst_value = unsafe { super::value::graphdb_value_to_core(dst_vid) };
    let src_id = match value_to_vertex_id(&src_value) {
        Some(id) => id,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };
    let dst_id = match value_to_vertex_id(&dst_value) {
        Some(id) => id,
        None => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    // Get edge type
    let edge_type_str = match CStr::from_ptr(edge_type).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return graphdb_error_code_t::GRAPHDB_MISUSE as c_int,
    };

    // Create edge
    let edge = Edge::new(src_id, dst_id, edge_type_str, 0, HashMap::new());
    handle.buffer.push(BatchItem::Edge(edge));

    // Auto-flush if buffer is full
    if handle.buffer.len() >= handle.batch_size {
        if let Err(e) = handle.execute() {
            set_last_error_message(e);
            return graphdb_error_code_t::GRAPHDB_ERROR as c_int;
        }
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Execute batch operation (flush all buffered data)
///
/// # Parameters
/// - `batch`: batch operation handle
///
/// # Returns
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_execute(batch: *mut graphdb_batch_t) -> c_int {
    if batch.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &mut *(batch as *mut GraphDbBatchHandle);

    // Check if session is valid
    if !handle.is_session_valid() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    if let Err(e) = handle.execute() {
        set_last_error_message(e);
        return graphdb_error_code_t::GRAPHDB_ERROR as c_int;
    }

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of vertices inserted
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `count`: output parameter
///
/// # Returns
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `count` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_vertices_inserted(
    batch: *mut graphdb_batch_t,
    count: *mut c_int,
) -> c_int {
    if batch.is_null() || count.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(batch as *mut GraphDbBatchHandle);
    *count = handle.vertices_inserted as c_int;

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of edges inserted
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `count`: output parameter
///
/// # Returns
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `count` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_edges_inserted(
    batch: *mut graphdb_batch_t,
    count: *mut c_int,
) -> c_int {
    if batch.is_null() || count.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(batch as *mut GraphDbBatchHandle);
    *count = handle.edges_inserted as c_int;

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of items in the buffer
///
/// # Parameters
/// - `batch`: batch operation handle
/// - `count`: output parameter
///
/// # Returns
/// Success: GRAPHDB_OK
/// Failure: Error code
///
/// # Safety
/// - `batch` must be a valid batch handle
/// - `count` must be a valid pointer
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_buffered_count(
    batch: *mut graphdb_batch_t,
    count: *mut c_int,
) -> c_int {
    if batch.is_null() || count.is_null() {
        return graphdb_error_code_t::GRAPHDB_MISUSE as c_int;
    }

    let handle = &*(batch as *mut GraphDbBatchHandle);
    *count = handle.buffer.len() as c_int;

    graphdb_error_code_t::GRAPHDB_OK as c_int
}

/// Get the number of buffered vertices
///
/// # Safety
/// - `batch` must be a valid batch handle or null
///
/// # Returns
/// - Number of buffered vertices, or -1 if batch is null
#[no_mangle]
pub unsafe extern "C" fn graphdb_batch_buffered_vertices(batch: *mut graphdb_batch_t) -> c_int {
    if batch.is_null() {
        return -1;
    }

    let handle = &*(batch as *mut GraphDbBatchHandle);
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
pub unsafe extern "C" fn graphdb_batch_buffered_edges(batch: *mut graphdb_batch_t) -> c_int {
    if batch.is_null() {
        return -1;
    }

    let handle = &*(batch as *mut GraphDbBatchHandle);
    handle
        .buffer
        .iter()
        .filter(|item| matches!(item, BatchItem::Edge(_)))
        .count() as c_int
}

fn value_to_vertex_id(value: &crate::core::Value) -> Option<VertexId> {
    use crate::core::Value;
    match value {
        Value::Int(i) => Some(VertexId::from_int64(*i as i64)),
        Value::BigInt(i) => Some(VertexId::from_int64(*i)),
        Value::String(s) => Some(VertexId::from_string(s)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_inserter_create_null_params() {
        let result = unsafe {
            graphdb_batch_inserter_create(std::ptr::null_mut(), 100, std::ptr::null_mut())
        };
        assert_eq!(result, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }

    #[test]
    fn test_batch_free_null() {
        // Should not panic
        unsafe { graphdb_batch_free(std::ptr::null_mut()) };
    }

    #[test]
    fn test_batch_buffered_counts_null() {
        let result =
            unsafe { graphdb_batch_buffered_count(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, graphdb_error_code_t::GRAPHDB_MISUSE as c_int);
    }
}

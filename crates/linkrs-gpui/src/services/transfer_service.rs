use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;
use crate::state::AppState;

/// Request payload for one server-side import.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    pub space: String,
    pub format: String,
    pub target_type: String,
    pub target_name: String,
    pub batch_size: Option<usize>,
    pub file_name: String,
    pub file_bytes: Vec<u8>,
}

/// Upload a local file for server-side import.
pub fn import_file(
    cx: &App,
    request: ImportRequest,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = request.space;
    let format = request.format;
    let target_type = request.target_type;
    let target_name = request.target_name;
    let batch_size = request.batch_size;
    let file_name = request.file_name;
    let file_bytes = request.file_bytes;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .server_import_file(
                &space,
                &format,
                &target_type,
                &target_name,
                batch_size,
                &file_name,
                file_bytes,
            )
            .await
    })
}

/// Poll a server-side import job.
pub fn import_status(cx: &App, job_id: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let job_id = job_id.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().import_status(&job_id).await
    })
}

/// Download a server-side export into memory.
pub fn export_bytes(
    cx: &App,
    query: &str,
    format: &str,
    space: Option<String>,
) -> Task<Result<Vec<u8>, ClientError>> {
    let manager = SessionStore::manager(cx);
    let query = query.to_string();
    let format = format.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard
            .client()
            .server_export_bytes(&query, id, &format, space.as_deref())
            .await
    })
}

/// Download a server-side export into a local file.
pub fn export_to_file(
    cx: &App,
    query: &str,
    format: &str,
    space: Option<String>,
    path: &str,
) -> Task<Result<usize, ClientError>> {
    let manager = SessionStore::manager(cx);
    let query = query.to_string();
    let format = format.to_string();
    let path = path.to_string();
    let session_id = AppState::global(cx).read(cx).session_id;
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let id = session_id.ok_or(ClientError::NotConnected)?;
        guard
            .client()
            .server_export_to_file(&query, id, &format, space.as_deref(), &path)
            .await
    })
}

use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// List registered functions.
pub fn list(cx: &App) -> Task<Result<Vec<String>, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.list_functions().await
    })
}

/// Get function details.
pub fn info(cx: &App, name: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_function_info(&name).await
    })
}

/// Register a custom function.
pub fn register(
    cx: &App,
    name: &str,
    implementation: &str,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    let implementation = implementation.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().register_function(&name, &implementation).await
    })
}

/// Unregister a custom function.
pub fn unregister(cx: &App, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().unregister_function(&name).await
    })
}

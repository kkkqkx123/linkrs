use gpui_kit::{App, Task};
use linkrs_client::ClientError;

use crate::connection::SessionStore;
use crate::runtime;

/// Get full server configuration.
pub fn get(cx: &App) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_config().await
    })
}

/// Get one configuration item.
pub fn get_key(cx: &App, section: &str, key: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let section = section.to_string();
    let key = key.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().get_config_key(&section, &key).await
    })
}

/// Update one configuration item.
pub fn update(
    cx: &App,
    section: &str,
    key: &str,
    value: serde_json::Value,
) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let section = section.to_string();
    let key = key.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().update_config(&section, &key, value).await
    })
}

/// Reset one configuration item.
pub fn reset(cx: &App, section: &str, key: &str) -> Task<Result<serde_json::Value, ClientError>> {
    let manager = SessionStore::manager(cx);
    let section = section.to_string();
    let key = key.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().reset_config(&section, &key).await
    })
}

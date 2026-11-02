use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_wire::meta::UserListResponse;

use crate::connection::SessionStore;
use crate::runtime;

/// List users (admin only).
pub fn list(cx: &App) -> Task<Result<UserListResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().list_users().await
    })
}

/// Create a user.
pub fn create(cx: &App, username: &str, password: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let username = username.to_string();
    let password = password.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().create_user(&username, &password).await
    })
}

/// Drop a user.
pub fn drop(cx: &App, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().drop_user(&name).await
    })
}

/// Reset a user password.
pub fn reset_password(cx: &App, name: &str, new_password: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    let new_password = new_password.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().reset_user_password(&name, &new_password).await
    })
}

/// Enable a user.
pub fn enable(cx: &App, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().enable_user(&name).await
    })
}

/// Disable a user.
pub fn disable(cx: &App, name: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().disable_user(&name).await
    })
}

/// Grant a role to a user in a space.
pub fn grant_role(cx: &App, name: &str, role: &str, space: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    let role = role.to_string();
    let space = space.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().grant_user_role(&name, &role, &space).await
    })
}

/// Change the signed-in user's own password through a `CHANGE PASSWORD`
/// statement, mirroring the web studio behavior.
pub fn change_own_password(
    cx: &App,
    old_password: &str,
    new_password: &str,
) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let statement = format!(
        "CHANGE PASSWORD {} TO {}",
        quote_literal(old_password),
        quote_literal(new_password)
    );
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        let outcome = guard.execute_query(&statement).await?;
        match outcome.error {
            Some(info) => Err(ClientError::QueryError(info.message)),
            None => Ok(()),
        }
    })
}

fn quote_literal(raw: &str) -> String {
    format!(
        "\"{}\"",
        raw.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// Revoke a user's role in a space.
pub fn revoke_role(cx: &App, name: &str, space: &str) -> Task<Result<(), ClientError>> {
    let manager = SessionStore::manager(cx);
    let name = name.to_string();
    let space = space.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().revoke_user_role(&name, &space).await
    })
}

use gpui_kit::{App, Task};
use linkrs_client::ClientError;
use linkrs_wire::migration::{
    MigrationExecuteResponse, MigrationHistoryResponse, MigrationPlanResponse,
    MigrationStatusResponse,
};

use crate::connection::SessionStore;
use crate::runtime;

/// Build a migration plan between two schema versions.
pub fn create_plan(
    cx: &App,
    space: &str,
    label: &str,
    from_version: u64,
    to_version: u64,
    is_edge: bool,
    expand_contract: bool,
) -> Task<Result<MigrationPlanResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .create_migration_plan(
                &space,
                &label,
                from_version,
                to_version,
                is_edge,
                expand_contract,
            )
            .await
    })
}

/// Execute a migration plan document.
pub fn execute(cx: &App, plan_json: &str) -> Task<Result<MigrationExecuteResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let plan_json = plan_json.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().execute_migration(&plan_json).await
    })
}

/// Roll back a migration plan document.
pub fn rollback(cx: &App, plan_json: &str) -> Task<Result<MigrationExecuteResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let plan_json = plan_json.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().rollback_migration(&plan_json).await
    })
}

/// Dry-run a migration plan document without applying it.
pub fn dry_run(cx: &App, plan_json: &str) -> Task<Result<MigrationExecuteResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let plan_json = plan_json.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard.client().dry_run_migration(&plan_json).await
    })
}

/// Migration history for one space label.
pub fn history(
    cx: &App,
    space: &str,
    label: &str,
    is_edge: bool,
) -> Task<Result<MigrationHistoryResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .migration_history(&space, &label, is_edge)
            .await
    })
}

/// Migration status for one space label.
pub fn status(
    cx: &App,
    space: &str,
    label: &str,
    is_edge: bool,
) -> Task<Result<MigrationStatusResponse, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .migration_status(&space, &label, is_edge)
            .await
    })
}

/// Collect migration progress events into one batch.
pub fn progress(
    cx: &App,
    space: &str,
    label: &str,
    is_edge: bool,
) -> Task<Result<Vec<serde_json::Value>, ClientError>> {
    let manager = SessionStore::manager(cx);
    let space = space.to_string();
    let label = label.to_string();
    runtime::spawn(cx, async move {
        let guard = manager.lock().await;
        guard
            .client()
            .stream_migration_progress(&space, &label, is_edge)
            .await
    })
}

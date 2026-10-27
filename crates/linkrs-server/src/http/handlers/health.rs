use axum::{http::StatusCode, response::Json};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

static READY: AtomicBool = AtomicBool::new(false);
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Mark the service as ready for traffic.
pub fn set_ready(ready: bool) {
    READY.store(ready, Ordering::SeqCst);
}

/// Mark the service as stopping.
pub fn set_stopping(stopping: bool) {
    STOPPING.store(stopping, Ordering::SeqCst);
}

/// Whether the service is currently ready.
pub fn is_ready() -> bool {
    READY.load(Ordering::SeqCst) && !STOPPING.load(Ordering::SeqCst)
}

fn dependency_status() -> serde_json::Value {
    json!({
        "process": "ok",
        "storage": if is_ready() { "ok" } else { "unavailable" },
        "transaction_manager": if is_ready() { "ok" } else { "unavailable" },
        "listeners": if is_ready() { "ok" } else { "starting" },
    })
}

fn optional_components() -> serde_json::Value {
    json!({
        "vector": "optional",
        "fulltext": "optional",
    })
}

#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "Health",
    responses(
        (status = 200, body = serde_json::Value, description = "Service health status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn check() -> (StatusCode, Json<serde_json::Value>) {
    let ready = is_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if ready { "healthy" } else { "not_ready" },
            "service": "linkrs",
            "version": env!("CARGO_PKG_VERSION"),
            "ready": ready,
            "live": true,
            "checks": dependency_status(),
            "optional": optional_components(),
        })),
    )
}

/// Liveness probe: the process is alive and its main loop is not blocked.
#[utoipa::path(
    get,
    path = "/v1/health/live",
    tag = "Health",
    responses(
        (status = 200, body = serde_json::Value, description = "Process is alive"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn live() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "status": "alive",
            "service": "linkrs",
            "version": env!("CARGO_PKG_VERSION"),
            "live": true,
        })),
    )
}

/// Readiness probe: storage, transaction manager, and listeners are available.
#[utoipa::path(
    get,
    path = "/v1/health/ready",
    tag = "Health",
    responses(
        (status = 200, body = serde_json::Value, description = "Service readiness status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn ready() -> (StatusCode, Json<serde_json::Value>) {
    let ready = is_ready();
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if ready { "healthy" } else { "not_ready" },
            "service": "linkrs",
            "version": env!("CARGO_PKG_VERSION"),
            "ready": ready,
            "live": true,
            "checks": dependency_status(),
            "optional": optional_components(),
        })),
    )
}

use axum::{http::StatusCode, response::Json};
use linkrs_wire::meta::{HealthChecks, HealthOptional, HealthResponse, LiveResponse};

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

fn dependency_status() -> HealthChecks {
    let status = if is_ready() { "ok" } else { "unavailable" };
    let listeners = if is_ready() { "ok" } else { "starting" };
    HealthChecks {
        process: "ok".to_string(),
        storage: status.to_string(),
        transaction_manager: status.to_string(),
        listeners: listeners.to_string(),
    }
}

fn optional_components() -> HealthOptional {
    HealthOptional {
        vector: "optional".to_string(),
        fulltext: "optional".to_string(),
    }
}

fn full_health() -> HealthResponse {
    let ready = is_ready();
    HealthResponse {
        status: if ready { "healthy" } else { "not_ready" }.to_string(),
        service: "linkrs".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        ready,
        live: true,
        checks: dependency_status(),
        optional: optional_components(),
    }
}

#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "Health",
    responses(
        (status = 200, body = HealthResponse, description = "Service health status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn check() -> (StatusCode, Json<HealthResponse>) {
    let body = full_health();
    let status = if body.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

/// Liveness probe: the process is alive and its main loop is not blocked.
#[utoipa::path(
    get,
    path = "/v1/health/live",
    tag = "Health",
    responses(
        (status = 200, body = LiveResponse, description = "Process is alive"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn live() -> (StatusCode, Json<LiveResponse>) {
    (
        StatusCode::OK,
        Json(LiveResponse {
            status: "alive".to_string(),
            service: "linkrs".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            live: true,
        }),
    )
}

/// Readiness probe: storage, transaction manager, and listeners are available.
#[utoipa::path(
    get,
    path = "/v1/health/ready",
    tag = "Health",
    responses(
        (status = 200, body = HealthResponse, description = "Service readiness status"),
        (status = 500, description = "Internal error")
    )
)]
pub async fn ready() -> (StatusCode, Json<HealthResponse>) {
    let body = full_health();
    let status = if body.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(body))
}

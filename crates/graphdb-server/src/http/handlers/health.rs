use axum::{http::StatusCode, response::Json};
use serde_json::json;

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
    (
        StatusCode::OK,
        Json(json!({
            "status": "healthy",
            "service": "graphdb",
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

use axum::{http::StatusCode, response::Json};
use serde_json::json;

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

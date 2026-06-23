//! Web Module Error Types

use axum::{
    http::StatusCode,
    response::{IntoResponse, Json},
};
use serde_json::json;
use thiserror::Error;

/// Web module error type
#[derive(Error, Debug)]
pub enum WebError {
    #[error("Storage error: {0}")]
    Storage(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Query error: {0}")]
    Query(String),
}

impl IntoResponse for WebError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_code, message) = match &self {
            WebError::Storage(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORAGE_ERROR",
                msg.clone(),
            ),
            WebError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "BAD_REQUEST", msg.clone()),
            WebError::NotFound(msg) => (StatusCode::NOT_FOUND, "NOT_FOUND", msg.clone()),
            WebError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED", msg.clone()),
            WebError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                msg.clone(),
            ),
            WebError::Query(msg) => (StatusCode::BAD_REQUEST, "QUERY_ERROR", msg.clone()),
        };

        let body = Json(json!({
            "success": false,
            "error": {
                "code": error_code,
                "message": message
            }
        }));

        (status, body).into_response()
    }
}

impl From<sqlx::Error> for WebError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => WebError::NotFound("Record not found".to_string()),
            _ => WebError::Storage(err.to_string()),
        }
    }
}

/// Web module result type
pub type WebResult<T> = Result<T, WebError>;

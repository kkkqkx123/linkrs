use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub enum HttpError {
    BadRequest(String),
    Unauthorized(String),
    NotFound(String),
    Conflict(String),
    InternalError(String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::BadRequest(msg) => write!(f, "Bad Request: {}", msg),
            HttpError::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            HttpError::NotFound(msg) => write!(f, "Not Found: {}", msg),
            HttpError::Conflict(msg) => write!(f, "Conflict: {}", msg),
            HttpError::InternalError(msg) => write!(f, "Internal Error: {}", msg),
        }
    }
}

impl std::error::Error for HttpError {}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            HttpError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            HttpError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            HttpError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            HttpError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            HttpError::InternalError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(json!({
            "error": message,
            "status": status.as_u16(),
        }));

        (status, body).into_response()
    }
}

impl HttpError {
    /// Generate a BadRequest error.
    pub fn bad_request<T: Into<String>>(msg: T) -> Self {
        HttpError::BadRequest(msg.into())
    }

    /// Generate a “NotFound” error.
    pub fn not_found<T: Into<String>>(msg: T) -> Self {
        HttpError::NotFound(msg.into())
    }

    /// An “Unauthorized” error was generated.
    pub fn unauthorized<T: Into<String>>(msg: T) -> Self {
        HttpError::Unauthorized(msg.into())
    }

    /// Generate an InternalError.
    pub fn internal<T: Into<String>>(msg: T) -> Self {
        HttpError::InternalError(msg.into())
    }

    /// Classify the stable transaction error code emitted by the transaction
    /// crate instead of collapsing every operation into HTTP 500.
    pub fn transaction_message<T: Into<String>>(message: T) -> Self {
        let message = message.into();
        if message.contains("[transaction_not_found]") {
            Self::NotFound(message)
        } else if message.contains("[transaction_not_owner]") {
            Self::Unauthorized(message)
        } else if message.contains("[write_transaction_conflict]") {
            Self::Conflict(message)
        } else if message.contains("[transaction_timeout]")
            || message.contains("[transaction_expired]")
        {
            Self::BadRequest(message)
        } else if message.contains("[invalid_state") || message.contains("[savepoint") {
            Self::Conflict(message)
        } else {
            Self::InternalError(message)
        }
    }
}

impl From<graphdb_api::core::CoreError> for HttpError {
    fn from(err: graphdb_api::core::CoreError) -> Self {
        use graphdb_api::core::CoreError;
        match err {
            CoreError::NotFound(msg) => HttpError::NotFound(msg),
            CoreError::InvalidParameter(msg) => HttpError::BadRequest(msg),
            CoreError::QueryExecutionFailed(msg) => HttpError::InternalError(msg),
            CoreError::TransactionFailed(msg) => HttpError::transaction_message(msg),
            CoreError::SchemaOperationFailed(msg) => HttpError::InternalError(msg),
            CoreError::StorageError(msg) => HttpError::InternalError(msg),
            CoreError::Internal(msg) => HttpError::InternalError(msg),
            CoreError::DetailedQueryError { message, .. } => HttpError::InternalError(message),
            CoreError::SyncError(msg) => HttpError::InternalError(msg),
            CoreError::VectorError(msg) => HttpError::InternalError(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HttpError;

    #[test]
    fn transaction_error_codes_survive_handler_context() {
        assert!(matches!(
            HttpError::transaction_message("Failed to commit: [transaction_not_owner] denied"),
            HttpError::Unauthorized(_)
        ));
        assert!(matches!(
            HttpError::transaction_message("Failed to commit: [write_transaction_conflict]"),
            HttpError::Conflict(_)
        ));
        assert!(matches!(
            HttpError::transaction_message("Failed to commit: [transaction_timeout]"),
            HttpError::BadRequest(_)
        ));
    }
}

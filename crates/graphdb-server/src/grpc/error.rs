//! gRPC error mapping.
//!
//! Converts domain errors and wire id parsing failures to `tonic::Status`.

use tonic::Status;

use graphdb_transaction::{TransactionError, TransactionErrorKind, TransactionId};

/// Parse a string session id carried on the wire into the numeric id.
pub(crate) fn parse_session_id(value: &str) -> Result<i64, Status> {
    value
        .parse::<i64>()
        .map_err(|_| Status::invalid_argument("session_id must be an integer"))
}

/// Session id is mandatory on every gRPC call that reaches an HTTP handler,
/// because the HTTP handler performs authorization against that identity.
pub(crate) fn require_session_id(raw: &str) -> Result<i64, Status> {
    if raw.is_empty() {
        return Err(Status::unauthenticated("session_id is required"));
    }
    parse_session_id(raw)
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_transaction_id(value: &str) -> Result<TransactionId, Status> {
    value
        .parse::<u64>()
        .map(TransactionId::from)
        .map_err(|_| Status::invalid_argument("transaction_id must be an unsigned integer"))
}

pub(crate) fn transaction_status(error: TransactionError) -> Status {
    let message = error.to_string();
    match error.kind() {
        TransactionErrorKind::TransactionNotFound => Status::not_found(message),
        TransactionErrorKind::TransactionNotOwner => Status::permission_denied(message),
        TransactionErrorKind::TransactionTimeout | TransactionErrorKind::TransactionExpired => {
            Status::deadline_exceeded(message)
        }
        TransactionErrorKind::WriteTransactionConflict => Status::aborted(message),
        TransactionErrorKind::CommitVetoed => Status::aborted(message),
        TransactionErrorKind::InvalidStateForCommit
        | TransactionErrorKind::InvalidStateForAbort
        | TransactionErrorKind::InvalidStateForExecution
        | TransactionErrorKind::InvalidStateTransition => Status::failed_precondition(message),
        TransactionErrorKind::SavepointNotFound
        | TransactionErrorKind::SavepointFailed
        | TransactionErrorKind::SavepointNotActive
        | TransactionErrorKind::NoSavepointsInTransaction => Status::failed_precondition(message),
        _ => Status::internal(message),
    }
}

pub(crate) fn op_error_to_status(
    error: crate::http::handlers::function::FunctionOpError,
) -> Status {
    use crate::http::handlers::function::FunctionOpError;
    match error {
        FunctionOpError::NotFound(message) => Status::not_found(message),
        FunctionOpError::Conflict(message) => Status::already_exists(message),
        FunctionOpError::Invalid(message) => Status::invalid_argument(message),
        FunctionOpError::Failed(message) => Status::internal(message),
    }
}

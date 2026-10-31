//! Mapping between `tonic::Status` and the crate [`ClientError`].

use crate::utils::error::{ClientError, GrpcErrorCode};

/// Convert a failed RPC status into a structured client error.
pub(super) fn status_error(status: tonic::Status) -> ClientError {
    ClientError::grpc_status(grpc_code(status.code()), status.message())
}

fn grpc_code(code: tonic::Code) -> GrpcErrorCode {
    match code {
        tonic::Code::Ok => GrpcErrorCode::Other,
        tonic::Code::Cancelled => GrpcErrorCode::Cancelled,
        tonic::Code::Unknown => GrpcErrorCode::Unknown,
        tonic::Code::InvalidArgument => GrpcErrorCode::InvalidArgument,
        tonic::Code::DeadlineExceeded => GrpcErrorCode::DeadlineExceeded,
        tonic::Code::NotFound => GrpcErrorCode::NotFound,
        tonic::Code::AlreadyExists => GrpcErrorCode::AlreadyExists,
        tonic::Code::PermissionDenied => GrpcErrorCode::PermissionDenied,
        tonic::Code::ResourceExhausted => GrpcErrorCode::ResourceExhausted,
        tonic::Code::FailedPrecondition => GrpcErrorCode::FailedPrecondition,
        tonic::Code::Aborted => GrpcErrorCode::Aborted,
        tonic::Code::OutOfRange => GrpcErrorCode::OutOfRange,
        tonic::Code::Unimplemented => GrpcErrorCode::Unimplemented,
        tonic::Code::Internal => GrpcErrorCode::Internal,
        tonic::Code::Unavailable => GrpcErrorCode::Unavailable,
        tonic::Code::DataLoss => GrpcErrorCode::DataLoss,
        tonic::Code::Unauthenticated => GrpcErrorCode::Unauthenticated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_error_preserves_code_and_message() {
        let error = status_error(tonic::Status::unauthenticated("bad credentials"));
        match error {
            ClientError::GrpcError { code, message } => {
                assert_eq!(code, GrpcErrorCode::Unauthenticated);
                assert!(message.contains("bad credentials"), "message: {}", message);
            }
            other => panic!("expected grpc error, got {:?}", other),
        }
    }

    #[test]
    fn grpc_codes_map_to_structured_values() {
        assert_eq!(
            grpc_code(tonic::Code::PermissionDenied),
            GrpcErrorCode::PermissionDenied
        );
        assert_eq!(grpc_code(tonic::Code::NotFound), GrpcErrorCode::NotFound);
        assert_eq!(
            grpc_code(tonic::Code::Unavailable),
            GrpcErrorCode::Unavailable
        );
        assert_eq!(grpc_code(tonic::Code::Unknown), GrpcErrorCode::Unknown);
    }
}

//! Permission Error Type
//!
//! Coverage of Permission Management Related Errors.
//!
//! ## Design
//!
//! `PermissionError` is a struct with boxed source error to keep size small (~24 bytes).
//! This follows the same pattern as `DBError`, `QueryError`, `StorageError`, and `SessionError`.

use std::error::Error;

use crate::core::error::codes::{ErrorCode, PublicError, ToPublicError};

/// Thread-safe boxed error type
type BoxedError = Box<dyn Error + Send + Sync>;

/// Permission operation result type alias
pub type PermissionResult<T> = Result<T, PermissionError>;

/// Permission error kind enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionErrorKind {
    InsufficientPermission,
    NoRoleInSpace,
    PermissionDenied,
    RoleNotFound,
    UserNotFound,
    GrantRoleFailed,
    RevokeRoleFailed,
    OnlyGodCanManageSpaces,
    OnlyGodCanManageUsers,
    SpaceIdRequired,
    SchemaSpaceIdRequired,
    SchemaWriteSpaceIdRequired,
    SchemaWritePermissionDenied,
    DataReadSpaceIdRequired,
    DataWriteSpaceIdRequired,
    GuestCannotWriteData,
    CannotReadUserInfo,
    RoleOperationSpaceIdRequired,
    RoleOperationTargetRoleRequired,
    OnlyAdminOrGodCanManageRoles,
    CannotGrantRole,
    CannotModifyOwnRole,
    ChangePasswordTargetUserRequired,
    CanOnlyChangeOwnPassword,
}

impl PermissionErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            PermissionErrorKind::InsufficientPermission => "insufficient_permission",
            PermissionErrorKind::NoRoleInSpace => "no_role_in_space",
            PermissionErrorKind::PermissionDenied => "permission_denied",
            PermissionErrorKind::RoleNotFound => "role_not_found",
            PermissionErrorKind::UserNotFound => "user_not_found",
            PermissionErrorKind::GrantRoleFailed => "grant_role_failed",
            PermissionErrorKind::RevokeRoleFailed => "revoke_role_failed",
            PermissionErrorKind::OnlyGodCanManageSpaces => "only_god_can_manage_spaces",
            PermissionErrorKind::OnlyGodCanManageUsers => "only_god_can_manage_users",
            PermissionErrorKind::SpaceIdRequired => "space_id_required",
            PermissionErrorKind::SchemaSpaceIdRequired => "schema_space_id_required",
            PermissionErrorKind::SchemaWriteSpaceIdRequired => "schema_write_space_id_required",
            PermissionErrorKind::SchemaWritePermissionDenied => "schema_write_permission_denied",
            PermissionErrorKind::DataReadSpaceIdRequired => "data_read_space_id_required",
            PermissionErrorKind::DataWriteSpaceIdRequired => "data_write_space_id_required",
            PermissionErrorKind::GuestCannotWriteData => "guest_cannot_write_data",
            PermissionErrorKind::CannotReadUserInfo => "cannot_read_user_info",
            PermissionErrorKind::RoleOperationSpaceIdRequired => "role_operation_space_id_required",
            PermissionErrorKind::RoleOperationTargetRoleRequired => {
                "role_operation_target_role_required"
            }
            PermissionErrorKind::OnlyAdminOrGodCanManageRoles => {
                "only_admin_or_god_can_manage_roles"
            }
            PermissionErrorKind::CannotGrantRole => "cannot_grant_role",
            PermissionErrorKind::CannotModifyOwnRole => "cannot_modify_own_role",
            PermissionErrorKind::ChangePasswordTargetUserRequired => {
                "change_password_target_user_required"
            }
            PermissionErrorKind::CanOnlyChangeOwnPassword => "can_only_change_own_password",
        }
    }
}

/// Permission Related Errors
///
/// Design principles:
/// 1. Small size: Uses boxed errors to keep struct size minimal (~24 bytes)
/// 2. Full context: Preserves error chain
/// 3. Clone support: Can be cloned for logging/propagation
#[derive(Debug)]
pub struct PermissionError {
    kind: PermissionErrorKind,
    message: String,
    source: Option<BoxedError>,
}

impl PermissionError {
    pub fn new(kind: PermissionErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source(mut self, source: BoxedError) -> Self {
        self.source = Some(source);
        self
    }

    pub fn kind(&self) -> PermissionErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    // Convenience constructors
    pub fn insufficient_permission() -> Self {
        Self::new(
            PermissionErrorKind::InsufficientPermission,
            "Insufficient permission",
        )
    }

    pub fn no_role_in_space(user: impl Into<String>, space_id: i64) -> Self {
        Self::new(
            PermissionErrorKind::NoRoleInSpace,
            format!("User {} has no role in space {}", user.into(), space_id),
        )
    }

    pub fn permission_denied(permission: impl Into<String>, user: impl Into<String>) -> Self {
        Self::new(
            PermissionErrorKind::PermissionDenied,
            format!(
                "Permission denied: {} for user {}",
                permission.into(),
                user.into()
            ),
        )
    }

    pub fn role_not_found(role: impl Into<String>) -> Self {
        Self::new(
            PermissionErrorKind::RoleNotFound,
            format!("Role not found: {}", role.into()),
        )
    }

    pub fn user_not_found(user: impl Into<String>) -> Self {
        Self::new(
            PermissionErrorKind::UserNotFound,
            format!("User not found: {}", user.into()),
        )
    }

    pub fn grant_role_failed(message: impl Into<String>) -> Self {
        Self::new(PermissionErrorKind::GrantRoleFailed, message)
    }

    pub fn revoke_role_failed(message: impl Into<String>) -> Self {
        Self::new(PermissionErrorKind::RevokeRoleFailed, message)
    }

    pub fn only_god_can_manage_spaces() -> Self {
        Self::new(
            PermissionErrorKind::OnlyGodCanManageSpaces,
            "Permission denied: only GOD role can create/delete spaces",
        )
    }

    pub fn only_god_can_manage_users() -> Self {
        Self::new(
            PermissionErrorKind::OnlyGodCanManageUsers,
            "Permission denied: only GOD role can manage users",
        )
    }

    pub fn space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::SpaceIdRequired,
            "Space ID required for read Space operation",
        )
    }

    pub fn schema_space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::SchemaSpaceIdRequired,
            "Space ID required for read Schema operation",
        )
    }

    pub fn schema_write_space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::SchemaWriteSpaceIdRequired,
            "Space ID required for write Schema operation",
        )
    }

    pub fn schema_write_permission_denied(space_id: i64, user: impl Into<String>) -> Self {
        Self::new(
            PermissionErrorKind::SchemaWritePermissionDenied,
            format!(
                "Schema write permission denied: user {} has insufficient privileges in space {}",
                user.into(),
                space_id
            ),
        )
    }

    pub fn data_read_space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::DataReadSpaceIdRequired,
            "Space ID required for read data operation",
        )
    }

    pub fn data_write_space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::DataWriteSpaceIdRequired,
            "Space ID required for write data operation",
        )
    }

    pub fn guest_cannot_write_data() -> Self {
        Self::new(
            PermissionErrorKind::GuestCannotWriteData,
            "Guest role has no permission to write data",
        )
    }

    pub fn cannot_read_user_info() -> Self {
        Self::new(
            PermissionErrorKind::CannotReadUserInfo,
            "No permission to read user information",
        )
    }

    pub fn role_operation_space_id_required() -> Self {
        Self::new(
            PermissionErrorKind::RoleOperationSpaceIdRequired,
            "Space ID required for role operation",
        )
    }

    pub fn role_operation_target_role_required() -> Self {
        Self::new(
            PermissionErrorKind::RoleOperationTargetRoleRequired,
            "Target role required for role operation",
        )
    }

    pub fn only_admin_or_god_can_manage_roles() -> Self {
        Self::new(
            PermissionErrorKind::OnlyAdminOrGodCanManageRoles,
            "Permission denied: only Admin or God can manage roles",
        )
    }

    pub fn cannot_grant_role(role: impl Into<String>) -> Self {
        Self::new(
            PermissionErrorKind::CannotGrantRole,
            format!("Permission denied: cannot grant role {}", role.into()),
        )
    }

    pub fn cannot_modify_own_role() -> Self {
        Self::new(
            PermissionErrorKind::CannotModifyOwnRole,
            "Cannot modify own role",
        )
    }

    pub fn change_password_target_user_required() -> Self {
        Self::new(
            PermissionErrorKind::ChangePasswordTargetUserRequired,
            "Target user required for change password operation",
        )
    }

    pub fn can_only_change_own_password() -> Self {
        Self::new(
            PermissionErrorKind::CanOnlyChangeOwnPassword,
            "Can only change own password",
        )
    }
}

impl std::fmt::Display for PermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.kind.as_str(), self.message)?;
        if let Some(ref source) = self.source {
            write!(f, "\n  Caused by: {}", source)?;
        }
        Ok(())
    }
}

impl Clone for PermissionError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            message: self.message.clone(),
            source: None,
        }
    }
}

impl std::error::Error for PermissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl ToPublicError for PermissionError {
    fn to_public_error(&self) -> PublicError {
        PublicError::new(self.to_error_code(), self.to_public_message())
    }

    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::PermissionDenied
    }

    fn to_public_message(&self) -> String {
        self.message.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permission_error_size() {
        assert!(
            std::mem::size_of::<PermissionError>() <= 64,
            "PermissionError should be small, got {} bytes",
            std::mem::size_of::<PermissionError>()
        );
    }

    #[test]
    fn test_permission_error_creation() {
        let err = PermissionError::role_not_found("admin");
        assert_eq!(err.kind(), PermissionErrorKind::RoleNotFound);
        assert!(err.message().contains("admin"));
    }
}

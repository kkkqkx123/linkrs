//! User administration.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// List users.
    pub async fn list_users(&self, mut request: ListUsersRequest) -> Result<ListUsersResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_users(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create user.
    pub async fn create_user(&self, mut request: CreateUserRequest) -> Result<CreateUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .create_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Drop user.
    pub async fn drop_user(&self, mut request: DropUserRequest) -> Result<DropUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .drop_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Reset user password.
    pub async fn reset_user_password(
        &self,
        mut request: ResetUserPasswordRequest,
    ) -> Result<ResetUserPasswordResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .reset_user_password(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Enable user.
    pub async fn enable_user(&self, mut request: EnableUserRequest) -> Result<EnableUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .enable_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Disable user.
    pub async fn disable_user(
        &self,
        mut request: DisableUserRequest,
    ) -> Result<DisableUserResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .disable_user(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Grant user role.
    pub async fn grant_user_role(
        &self,
        mut request: GrantUserRoleRequest,
    ) -> Result<GrantUserRoleResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .grant_user_role(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Revoke user role.
    pub async fn revoke_user_role(
        &self,
        mut request: RevokeUserRoleRequest,
    ) -> Result<RevokeUserRoleResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .revoke_user_role(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}

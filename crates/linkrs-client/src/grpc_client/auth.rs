//! Health check, authentication, and server-side session management.

use crate::utils::error::Result;

use super::error::status_error;
use super::proto::*;

impl super::GrpcClient {
    /// Check server health, version, and uptime.
    pub async fn health_check(&self) -> Result<HealthCheckResponse> {
        let mut client = self.client();
        client
            .health_check(self.authed(tonic::Request::new(HealthCheckRequest {})))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Authenticate and remember the returned session id for later calls.
    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse> {
        self.login_in_space(username, password, None).await
    }

    /// Authenticate against a specific space and remember the session id.
    pub async fn login_in_space(
        &self,
        username: &str,
        password: &str,
        space: Option<&str>,
    ) -> Result<LoginResponse> {
        let request = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
            space: space.map(str::to_string),
        };
        let mut client = self.client();
        let response = client
            .login(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)?;
        if response.success {
            self.store_session(response.session_id.clone());
        }
        Ok(response)
    }

    /// End the remembered session and drop the stored session id.
    pub async fn logout(&self) -> Result<LogoutResponse> {
        let session_id = self.require_session()?;
        let mut client = self.client();
        let result = client
            .logout(self.authed(tonic::Request::new(LogoutRequest { session_id })))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error);
        self.clear_credentials();
        result
    }

    /// Fetch identity information for the remembered session.
    pub async fn auth_me(&self) -> Result<AuthMeResponse> {
        let session_id = self.require_session()?;
        let mut client = self.client();
        client
            .auth_me(self.authed(tonic::Request::new(AuthMeRequest { session_id })))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Create a server-side session and remember its id on success.
    pub async fn create_session(
        &self,
        request: CreateSessionRequest,
    ) -> Result<CreateSessionResponse> {
        let mut client = self.client();
        let response = client
            .create_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)?;
        if response.success && !response.session_id.is_empty() {
            self.store_session(response.session_id.clone());
        }
        Ok(response)
    }

    /// Inspect a server-side session.
    pub async fn get_session(&self, mut request: GetSessionRequest) -> Result<GetSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .get_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Close a server-side session.
    pub async fn close_session(
        &self,
        mut request: CloseSessionRequest,
    ) -> Result<CloseSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .close_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List active server-side sessions.
    pub async fn list_sessions(
        &self,
        mut request: ListSessionsRequest,
    ) -> Result<ListSessionsResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_sessions(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Terminate another session as an administrator.
    pub async fn kill_session(
        &self,
        mut request: KillSessionRequest,
    ) -> Result<KillSessionResponse> {
        self.fill_required_session(&mut request.session_id)?;
        let mut client = self.client();
        client
            .kill_session(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// List variables of a session.
    pub async fn list_session_variables(
        &self,
        mut request: ListSessionVariablesRequest,
    ) -> Result<ListSessionVariablesResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .list_session_variables(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Set a session variable.
    pub async fn set_session_variable(
        &self,
        mut request: SetSessionVariableRequest,
    ) -> Result<SetSessionVariableResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .set_session_variable(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }

    /// Delete a session variable.
    pub async fn delete_session_variable(
        &self,
        mut request: DeleteSessionVariableRequest,
    ) -> Result<DeleteSessionVariableResponse> {
        self.fill_optional_session_i64(&mut request.session_id)?;
        let mut client = self.client();
        client
            .delete_session_variable(self.authed(tonic::Request::new(request)))
            .await
            .map(|response| response.into_inner())
            .map_err(status_error)
    }
}

//! Session and authentication handlers.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::convert::system_time_secs;
use super::error::parse_session_id;
use super::proto::*;
use super::service::LinkrsService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > LinkrsService<S>
{
    pub(crate) async fn handle_login(
        &self,
        request: Request<LoginRequest>,
    ) -> Result<Response<LoginResponse>, Status> {
        let req = request.into_inner();
        if req.username.is_empty() || req.password.is_empty() {
            return Err(Status::unauthenticated(
                "username and password must not be empty",
            ));
        }
        let graph_service = self.app_state.server.get_graph_service();
        let session = graph_service
            .authenticate(&req.username, &req.password)
            .await
            .map_err(|message| {
                if message.to_lowercase().contains("locked") {
                    Status::permission_denied(message)
                } else {
                    Status::unauthenticated(message)
                }
            })?;
        if let Some(space) = req.space.filter(|s| !s.is_empty()) {
            attach_session_space(&self.app_state, &session, &space)?;
        }
        Ok(Response::new(LoginResponse {
            success: true,
            session_id: session.id().to_string(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_logout(
        &self,
        request: Request<LogoutRequest>,
    ) -> Result<Response<LogoutResponse>, Status> {
        let session_id = parse_session_id(&request.into_inner().session_id)?;
        self.app_state
            .server
            .get_session_manager()
            .remove_session(session_id)
            .await;
        Ok(Response::new(LogoutResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<CreateSessionResponse>, Status> {
        let req = request.into_inner();
        if req.username.is_empty() {
            return Err(Status::invalid_argument("username must not be empty"));
        }
        let session_manager = self.app_state.server.get_session_manager();
        let session = if req.password.is_empty() {
            let graph_service = self.app_state.server.get_graph_service();
            if graph_service.is_user_locked(&req.username) {
                return Err(Status::permission_denied(format!(
                    "account {} is locked",
                    req.username
                )));
            }
            session_manager
                .create_session(req.username.clone(), "127.0.0.1".to_string())
                .await
                .map_err(|e| Status::internal(format!("failed to create session: {e}")))?
        } else {
            let graph_service = self.app_state.server.get_graph_service();
            graph_service
                .authenticate(&req.username, &req.password)
                .await
                .map_err(|message| {
                    if message.to_lowercase().contains("locked") {
                        Status::permission_denied(message)
                    } else {
                        Status::unauthenticated(message)
                    }
                })?
        };
        if let Some(space) = req.space.filter(|s| !s.is_empty()) {
            attach_session_space(&self.app_state, &session, &space)?;
        }
        let space_id = session.space().map(|s| s.id as i32).unwrap_or(0);
        Ok(Response::new(CreateSessionResponse {
            success: true,
            session_id: session.id().to_string(),
            space_id,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let raw_id = request.into_inner().session_id;
        let session_id = parse_session_id(&raw_id)?;
        let session_manager = self.app_state.server.get_session_manager();
        match session_manager.get_session_info(session_id).await {
            None => Ok(Response::new(GetSessionResponse {
                exists: false,
                session_id: raw_id,
                username: String::new(),
                space_id: 0,
                created_at: 0,
                last_accessed: 0,
            })),
            Some(info) => {
                let space_id = session_manager
                    .find_session(session_id)
                    .and_then(|s| s.space())
                    .map(|s| s.id as i32)
                    .unwrap_or(0);
                Ok(Response::new(GetSessionResponse {
                    exists: true,
                    session_id: raw_id,
                    username: info.user_name,
                    space_id,
                    created_at: system_time_secs(&info.create_time),
                    last_accessed: system_time_secs(&info.last_access_time),
                }))
            }
        }
    }

    pub(crate) async fn handle_close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        let session_id = parse_session_id(&request.into_inner().session_id)?;
        self.app_state
            .server
            .get_session_manager()
            .remove_session(session_id)
            .await;
        Ok(Response::new(CloseSessionResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_list_session_variables(
        &self,
        request: Request<ListSessionVariablesRequest>,
    ) -> Result<Response<ListSessionVariablesResponse>, Status> {
        let session_id = request.into_inner().session_id;
        let session = self
            .app_state
            .server
            .get_session_manager()
            .find_session(session_id)
            .ok_or_else(|| Status::not_found("Session not found"))?;
        let snapshot = session.variables_snapshot();
        let variables: std::collections::HashMap<String, serde_json::Value> = snapshot
            .into_iter()
            .map(|(k, v)| (k, crate::value::to_json(v)))
            .collect();
        Ok(Response::new(ListSessionVariablesResponse {
            success: true,
            variables_json: serde_json::to_string(&variables).unwrap_or_default(),
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_set_session_variable(
        &self,
        request: Request<SetSessionVariableRequest>,
    ) -> Result<Response<SetSessionVariableResponse>, Status> {
        let req = request.into_inner();
        if req.name.trim().is_empty() {
            return Err(Status::invalid_argument("variable name must not be empty"));
        }
        let session = self
            .app_state
            .server
            .get_session_manager()
            .find_session(req.session_id)
            .ok_or_else(|| Status::not_found("Session not found"))?;
        let value_json: serde_json::Value = if req.value_json.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&req.value_json)
                .map_err(|e| Status::invalid_argument(format!("invalid value_json: {e}")))?
        };
        session.set_variable(req.name, crate::value::from_json(&value_json));
        Ok(Response::new(SetSessionVariableResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_delete_session_variable(
        &self,
        request: Request<DeleteSessionVariableRequest>,
    ) -> Result<Response<DeleteSessionVariableResponse>, Status> {
        let req = request.into_inner();
        let session = self
            .app_state
            .server
            .get_session_manager()
            .find_session(req.session_id)
            .ok_or_else(|| Status::not_found("Session not found"))?;
        if !session.remove_variable(&req.name) {
            return Err(Status::not_found(format!(
                "Variable '{}' not found",
                req.name
            )));
        }
        Ok(Response::new(DeleteSessionVariableResponse {
            success: true,
            error: String::new(),
        }))
    }
}

/// Attach an authenticated session to a space by name.
pub(crate) fn attach_session_space<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
>(
    app_state: &crate::http::AppState<S>,
    session: &std::sync::Arc<crate::client::ClientSession>,
    space: &str,
) -> Result<(), Status> {
    let storage = app_state.server.get_storage();
    let info = storage
        .read()
        .get_space(space)
        .map_err(|e| Status::internal(format!("failed to resolve space: {e}")))?
        .ok_or_else(|| Status::not_found(format!("space '{space}' not found")))?;
    session.set_space(linkrs_core::types::SpaceSummary::new(
        info.space_id,
        info.space_name,
        info.vid_type,
    ));
    Ok(())
}

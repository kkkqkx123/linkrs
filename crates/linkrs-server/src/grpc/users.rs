//! gRPC user, session-inspection, and auth-identity handlers.
//!
//! Parity with the HTTP admin channel (`/v1/users/*`, `GET /v1/sessions`,
//! `DELETE /v1/sessions/{id}`, `GET /v1/auth/me`). Every RPC carries the
//! caller `session_id` explicitly because gRPC has no HTTP auth-middleware
//! extension; admin-only calls reuse the same `GraphService::admin_*`
//! gates as the HTTP handlers.

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::convert::system_time_secs;
use super::error::{parse_session_id, require_session_id};
use super::proto::*;
use super::service::LinkrsService;

fn map_admin_error(message: String) -> Status {
    let lower = message.to_lowercase();
    if lower.contains("not found") {
        Status::not_found(message)
    } else if lower.contains("already exists") {
        Status::already_exists(message)
    } else if lower.contains("permission") || lower.contains("admin") {
        Status::permission_denied(message)
    } else {
        Status::invalid_argument(message)
    }
}

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
    fn caller_username(&self, session_id: i64) -> Result<String, Status> {
        self.app_state
            .server
            .get_session_manager()
            .find_session(session_id)
            .map(|session| session.user())
            .ok_or_else(|| Status::unauthenticated("Session not found"))
    }

    pub(crate) async fn handle_list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let caller_id = require_session_id(&request.into_inner().session_id)?;
        let caller = self.caller_username(caller_id)?;
        if !self
            .app_state
            .server
            .get_graph_service()
            .get_permission_manager()
            .is_admin(&caller)
        {
            return Err(Status::permission_denied("Admin permission required"));
        }
        let sessions = self
            .app_state
            .server
            .get_session_manager()
            .list_sessions()
            .await;
        let entries = sessions
            .into_iter()
            .map(|info| SessionEntry {
                session_id: info.session_id.to_string(),
                username: info.user_name,
                space_name: info.space_name.unwrap_or_default(),
                created_at: system_time_secs(&info.create_time) as i64,
                last_accessed: system_time_secs(&info.last_access_time) as i64,
            })
            .collect();
        Ok(Response::new(ListSessionsResponse {
            sessions: entries,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_kill_session(
        &self,
        request: Request<KillSessionRequest>,
    ) -> Result<Response<KillSessionResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let target_id = parse_session_id(&req.target_session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .kill_session(target_id, &caller)
            .await
            .map_err(|e| {
                let message = e.to_string();
                if message.to_lowercase().contains("not found") {
                    Status::not_found(message)
                } else if message.to_lowercase().contains("permission") {
                    Status::permission_denied(message)
                } else {
                    Status::internal(message)
                }
            })?;
        Ok(Response::new(KillSessionResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_auth_me(
        &self,
        request: Request<AuthMeRequest>,
    ) -> Result<Response<AuthMeResponse>, Status> {
        let caller_id = require_session_id(&request.into_inner().session_id)?;
        let session = self
            .app_state
            .server
            .get_session_manager()
            .find_session(caller_id)
            .ok_or_else(|| Status::unauthenticated("Session not found"))?;
        let username = session.user();
        let graph_service = self.app_state.server.get_graph_service();
        let permission_manager = graph_service.get_permission_manager();
        let roles: Vec<String> = permission_manager
            .list_user_roles(&username)
            .into_iter()
            .map(|(_, role)| role.to_string())
            .collect();
        let role = permission_manager
            .highest_role(&username)
            .map(|role| role.to_string())
            .unwrap_or_default();
        Ok(Response::new(AuthMeResponse {
            username,
            role,
            roles,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_list_users(
        &self,
        request: Request<ListUsersRequest>,
    ) -> Result<Response<ListUsersResponse>, Status> {
        let caller_id = require_session_id(&request.into_inner().session_id)?;
        let caller = self.caller_username(caller_id)?;
        let graph_service = self.app_state.server.get_graph_service();
        if !graph_service.get_permission_manager().is_admin(&caller) {
            return Err(Status::permission_denied("Admin permission required"));
        }
        let users = graph_service
            .list_users_detailed()
            .into_iter()
            .map(|detail| UserEntry {
                username: detail.username,
                role: detail.role.unwrap_or_default(),
                status: detail.status,
                last_active: detail.last_active.unwrap_or_default(),
            })
            .collect();
        Ok(Response::new(ListUsersResponse {
            users,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_create_user(
        &self,
        request: Request<CreateUserRequest>,
    ) -> Result<Response<CreateUserResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_create_user(&caller, &req.username, &req.password)
            .map_err(map_admin_error)?;
        Ok(Response::new(CreateUserResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_drop_user(
        &self,
        request: Request<DropUserRequest>,
    ) -> Result<Response<DropUserResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_drop_user(&caller, &req.username)
            .await
            .map_err(map_admin_error)?;
        Ok(Response::new(DropUserResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_reset_user_password(
        &self,
        request: Request<ResetUserPasswordRequest>,
    ) -> Result<Response<ResetUserPasswordResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_reset_password(&caller, Some(caller_id), &req.username, &req.new_password)
            .await
            .map_err(map_admin_error)?;
        Ok(Response::new(ResetUserPasswordResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_enable_user(
        &self,
        request: Request<EnableUserRequest>,
    ) -> Result<Response<EnableUserResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_set_user_enabled(&caller, Some(caller_id), &req.username, true)
            .await
            .map_err(map_admin_error)?;
        Ok(Response::new(EnableUserResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_disable_user(
        &self,
        request: Request<DisableUserRequest>,
    ) -> Result<Response<DisableUserResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_set_user_enabled(&caller, Some(caller_id), &req.username, false)
            .await
            .map_err(map_admin_error)?;
        Ok(Response::new(DisableUserResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_grant_user_role(
        &self,
        request: Request<GrantUserRoleRequest>,
    ) -> Result<Response<GrantUserRoleResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_grant_role(&caller, &req.username, &req.space, &req.role)
            .map_err(map_admin_error)?;
        Ok(Response::new(GrantUserRoleResponse {
            success: true,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_revoke_user_role(
        &self,
        request: Request<RevokeUserRoleRequest>,
    ) -> Result<Response<RevokeUserRoleResponse>, Status> {
        let req = request.into_inner();
        let caller_id = require_session_id(&req.session_id)?;
        let caller = self.caller_username(caller_id)?;
        self.app_state
            .server
            .get_graph_service()
            .admin_revoke_role(&caller, &req.username, &req.space)
            .map_err(map_admin_error)?;
        Ok(Response::new(RevokeUserRoleResponse {
            success: true,
            error: String::new(),
        }))
    }
}

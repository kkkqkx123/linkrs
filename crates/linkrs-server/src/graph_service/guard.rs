use linkrs_core::types::SpaceSummary;
use linkrs_core::Permission;

use super::context::QueryExecutionContext;
use super::error::GraphServiceError;
use super::GraphService;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > GraphService<S>
{
    /// Whether the statement is a server-resolved configuration statement.
    ///
    /// Shared by permission classification and the streaming entry guard so
    /// both agree on the set: configuration intents resolve on the
    /// materialized path only.
    pub(crate) fn is_config_statement(stmt: &str) -> bool {
        match Self::parse_stmt(stmt) {
            Some(crate::query::parser::ast::Stmt::ShowConfigs(_))
            | Some(crate::query::parser::ast::Stmt::UpdateConfigs(_)) => true,
            _ => false,
        }
    }

    /// Permission required by a parsed statement for the space gate.
    ///
    /// Returns `None` for statements handled outside the space gate:
    /// session commands and access-control statements with a dedicated path.
    ///
    /// Derivation input is the parsed [`Stmt`](crate::query::parser::ast::Stmt),
    /// grouped by [`StmtCategory`](crate::query::parser::ast::StmtCategory):
    /// DML writes (INSERT/UPDATE/UPSERT/MERGE/SET/REMOVE, plus CHECKPOINT and
    /// COPY FROM) require Write, DELETE requires Delete; DDL schema objects
    /// (TAG/EDGE/INDEX/SEQUENCE, ALTER, full-text/vector indexes, CLEAR SPACE,
    /// PURGE EXPIRED, migrations and database attach/detach) require Schema
    /// while CREATE/DROP SPACE require Admin; DQL and search reads require
    /// Read; server configuration (SHOW/UPDATE CONFIGS, KILL QUERY, SHOW
    /// SESSIONS, extensions) requires Admin while SHOW QUERIES stays Read. The HTTP probe handlers in
    /// `crate::http::handlers::authz` mirror this table through
    /// [`PermissionChecker`](crate::permission::PermissionChecker); both sides
    /// must stay aligned (see `dcl-permission-matrix-drift`).
    pub(crate) fn permission_for_stmt(
        stmt: &crate::query::parser::ast::Stmt,
    ) -> Option<Permission> {
        use crate::query::parser::ast::Stmt as S;
        match stmt {
            S::Query(query) => {
                let mut current: Option<Permission> = None;
                for inner in &query.statements {
                    match Self::permission_for_stmt(inner) {
                        None => return None,
                        Some(next) => {
                            current = Some(match current {
                                None => next,
                                Some(prev) => Self::stronger_permission(prev, next),
                            });
                        }
                    }
                }
                current.or(Some(Permission::Read))
            }
            S::Pipe(pipe) => {
                let left = Self::permission_for_stmt(&pipe.left);
                let right = Self::permission_for_stmt(&pipe.right);
                match (left, right) {
                    (None, _) | (_, None) => None,
                    (Some(a), Some(b)) => Some(Self::stronger_permission(a, b)),
                }
            }
            S::Assignment(assign) => Self::permission_for_stmt(&assign.statement),
            S::SetOperation(setop) => {
                let left = Self::permission_for_stmt(&setop.left);
                let right = Self::permission_for_stmt(&setop.right);
                match (left, right) {
                    (None, _) | (_, None) => None,
                    (Some(a), Some(b)) => Some(Self::stronger_permission(a, b)),
                }
            }
            S::Explain(explain) => Self::permission_for_stmt(&explain.statement),
            S::Profile(profile) => Self::permission_for_stmt(&profile.statement),
            S::CreateUser(_)
            | S::AlterUser(_)
            | S::DropUser(_)
            | S::ChangePassword(_)
            | S::Grant(_)
            | S::Revoke(_)
            | S::DescribeUser(_)
            | S::ShowUsers(_)
            | S::ShowRoles(_) => None,
            S::Use(_) | S::AssignVariable(_) | S::BeginTransaction(_)
            | S::CommitTransaction(_) | S::RollbackTransaction(_) | S::Savepoint(_)
            | S::ReleaseSavepoint(_) => None,
            S::Match(matched) => {
                if matched.delete_clause.is_some() {
                    Some(Permission::Delete)
                } else {
                    Some(Permission::Read)
                }
            }
            // DQL and search reads (StmtCategory::Dql / Search).
            S::Go(_)
            | S::Fetch(_)
            | S::Lookup(_)
            | S::Subgraph(_)
            | S::FindPath(_)
            | S::GroupBy(_)
            | S::Unwind(_)
            | S::Return(_)
            | S::With(_)
            | S::Yield(_)
            | S::Filter(_)
            | S::Collect(_)
            | S::Show(_)
            | S::Desc(_)
            | S::ShowCreate(_)
            | S::ShowQueries(_)
            | S::Search(_)
            | S::LookupFulltext(_)
            | S::MatchFulltext(_)
            | S::LookupVector(_)
            | S::MatchVector(_)
            | S::SearchVector(_)
            | S::ShowFulltextIndex(_)
            | S::DescribeFulltextIndex(_)
            | S::LoadFrom(_)
            | S::InQueryCall(_)
            | S::ExportDatabase(_) => Some(Permission::Read),
            // DML writes (StmtCategory::Dml). `Update` covers both UPDATE and
            // UPSERT spellings (`is_upsert` only changes the write mode), so
            // neither falls through to the read default; MERGE is a write likewise.
            S::Insert(_)
            | S::Update(_)
            | S::Merge(_)
            | S::Set(_)
            | S::Remove(_)
            | S::Checkpoint(_) => Some(Permission::Write),
            S::Copy(copy) => match copy.direction {
                crate::query::parser::ast::CopyDirection::From => Some(Permission::Write),
                crate::query::parser::ast::CopyDirection::To => Some(Permission::Read),
            },
            S::Delete(_) => Some(Permission::Delete),
            // Schema objects (StmtCategory::Ddl): space creation is an Admin
            // operation, data-shape creation stays at Write, and every other
            // schema object (TAG/EDGE types, indexes, sequences, AS-query
            // definitions) requires Schema so USER cannot create them.
            S::Create(create) => Some(match &create.target {
                crate::query::parser::ast::CreateTarget::Space { .. } => Permission::Admin,
                crate::query::parser::ast::CreateTarget::Node { .. }
                | crate::query::parser::ast::CreateTarget::Edge { .. }
                | crate::query::parser::ast::CreateTarget::Path { .. } => Permission::Write,
                crate::query::parser::ast::CreateTarget::Tag { .. }
                | crate::query::parser::ast::CreateTarget::EdgeType { .. }
                | crate::query::parser::ast::CreateTarget::Index { .. }
                | crate::query::parser::ast::CreateTarget::Sequence { .. }
                | crate::query::parser::ast::CreateTarget::TagAsQuery { .. }
                | crate::query::parser::ast::CreateTarget::EdgeAsQuery { .. } => {
                    Permission::Schema
                }
            }),
            S::Drop(drop) => Some(match &drop.target {
                crate::query::parser::ast::DropTarget::Space(_) => Permission::Admin,
                crate::query::parser::ast::DropTarget::Tags(_)
                | crate::query::parser::ast::DropTarget::Edges(_)
                | crate::query::parser::ast::DropTarget::TagIndex { .. }
                | crate::query::parser::ast::DropTarget::EdgeIndex { .. }
                | crate::query::parser::ast::DropTarget::Sequence(_) => Permission::Schema,
            }),
            S::Alter(_)
            | S::CreateFulltextIndex(_)
            | S::DropFulltextIndex(_)
            | S::AlterFulltextIndex(_)
            | S::CreateVectorIndex(_)
            | S::DropVectorIndex(_)
            | S::CreateMacro(_)
            | S::DropMacro(_)
            | S::CreateType(_)
            | S::DropType(_)
            | S::ClearSpace(_)
            | S::PurgeExpired(_)
            | S::Migrate(_)
            | S::ImportDatabase(_)
            | S::AttachDatabase(_)
            | S::DetachDatabase(_)
            | S::Analyze(_) => Some(Permission::Schema),
            S::ShowConfigs(_)
            | S::UpdateConfigs(_)
            | S::KillQuery(_)
            | S::ShowSessions(_)
            | S::Extension(_) => Some(Permission::Admin),
        }
    }

    fn stronger_permission(first: Permission, second: Permission) -> Permission {
        use Permission as P;
        let rank = |perm: Permission| match perm {
            P::Read => 0,
            P::Write => 1,
            P::Delete => 1,
            P::Schema => 2,
            P::Admin => 3,
        };
        if rank(second) > rank(first) {
            second
        } else {
            first
        }
    }

    fn require_permission(
        &self,
        username: &str,
        space_id: i64,
        permission: Permission,
    ) -> Result<(), GraphServiceError> {
        if self.permission_manager.is_admin(username) {
            return Ok(());
        }
        self.permission_manager
            .check_permission(username, space_id, permission)
            .map_err(|e| GraphServiceError::new(format!("Permission check failed: {}", e)))
    }

    /// Whether the parsed statement switches the session space.
    ///
    /// Session-space binding keys off the parsed variant, never the raw text:
    /// the parser accepts any whitespace between `USE` and the space name, so a
    /// text prefix would miss those spellings and leave the session unrouted.
    pub(crate) fn is_use_statement(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(stmt, crate::query::parser::ast::Stmt::Use(_))
    }

    /// Whether the parsed statement is answered from the live session or
    /// query registries instead of the query engine.
    pub(crate) fn is_session_inspection(stmt: &crate::query::parser::ast::Stmt) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::ShowSessions(_)
                | crate::query::parser::ast::Stmt::ShowQueries(_)
                | crate::query::parser::ast::Stmt::KillQuery(_)
        )
    }

    /// Whether a parsed statement writes user state and needs post-execution alignment.
    pub(crate) fn is_user_management_write_stmt(
        stmt: &crate::query::parser::ast::Stmt,
    ) -> bool {
        matches!(
            stmt,
            crate::query::parser::ast::Stmt::CreateUser(_)
                | crate::query::parser::ast::Stmt::AlterUser(_)
                | crate::query::parser::ast::Stmt::DropUser(_)
                | crate::query::parser::ast::Stmt::ChangePassword(_)
                | crate::query::parser::ast::Stmt::Grant(_)
                | crate::query::parser::ast::Stmt::Revoke(_)
        )
    }

    /// Shared permission gate for regular statements and transaction commands.
    /// Session commands and access-control statements bypass the space gate
    /// through a dedicated path; every other statement is classified from
    /// the parsed form and unparseable text is rejected.
    pub(crate) fn check_query_permission(
        &self,
        username: &str,
        context: &QueryExecutionContext<'_>,
    ) -> Result<(), GraphServiceError> {
        if self.is_user_locked(username) {
            return Err(GraphServiceError::new(
                "Permission check failed: account is locked".to_string(),
            ));
        }
        if self.must_change_password(username) && !Self::is_change_password_intent(context.stmt) {
            return Err(GraphServiceError::new(
                "Permission check failed: password change required before other operations"
                    .to_string(),
            ));
        }
        let resolved = context
            .parsed_ast
            .as_ref()
            .map(|parsed| parsed.stmt().clone())
            .or_else(|| Self::parse_stmt(context.stmt));
        let Some(stmt) = resolved else {
            return Err(GraphServiceError::new(
                "Permission check failed: unable to classify statement".to_string(),
            ));
        };
        self.enforce_stmt_permission(username, &stmt, context.space_id)?;
        // Password policy applies to statements the caller is allowed to run:
        // an unauthorized caller learns the denial, not the policy details.
        self.enforce_new_password_policy(username, &stmt)
    }

    fn enforce_stmt_permission(
        &self,
        username: &str,
        stmt: &crate::query::parser::ast::Stmt,
        space_id: i64,
    ) -> Result<(), GraphServiceError> {
        use crate::query::parser::ast::Stmt as S;
        match stmt {
            S::CreateUser(_)
            | S::AlterUser(_)
            | S::DropUser(_)
            | S::Grant(_)
            | S::Revoke(_)
            | S::ChangePassword(_)
            | S::ShowUsers(_)
            | S::ShowRoles(_)
            | S::DescribeUser(_) => self.enforce_user_management(username, stmt, space_id),
            S::Use(_) | S::AssignVariable(_) | S::BeginTransaction(_)
            | S::CommitTransaction(_) | S::RollbackTransaction(_) | S::Savepoint(_)
            | S::ReleaseSavepoint(_) => Ok(()),
            S::Query(query) => {
                for inner in &query.statements {
                    self.enforce_stmt_permission(username, inner, space_id)?;
                }
                Ok(())
            }
            S::Pipe(pipe) => {
                self.enforce_stmt_permission(username, &pipe.left, space_id)?;
                self.enforce_stmt_permission(username, &pipe.right, space_id)
            }
            S::Assignment(assign) => {
                self.enforce_stmt_permission(username, &assign.statement, space_id)
            }
            S::SetOperation(setop) => {
                self.enforce_stmt_permission(username, &setop.left, space_id)?;
                self.enforce_stmt_permission(username, &setop.right, space_id)
            }
            S::Explain(explain) => {
                self.enforce_stmt_permission(username, &explain.statement, space_id)
            }
            S::Profile(profile) => {
                self.enforce_stmt_permission(username, &profile.statement, space_id)
            }
            other => match Self::permission_for_stmt(other) {
                None => Err(GraphServiceError::new(
                    "Permission check failed: unable to classify statement".to_string(),
                )),
                Some(permission) => self.require_permission(username, space_id, permission),
            },
        }
    }

    fn is_change_password_intent(stmt: &str) -> bool {
        matches!(
            Self::parse_stmt(stmt),
            Some(crate::query::parser::ast::Stmt::ChangePassword(_))
        )
    }

    pub(crate) fn parse_stmt(stmt: &str) -> Option<crate::query::parser::ast::Stmt> {
        let mut parser = crate::query::parser::Parser::new(stmt);
        match parser.parse() {
            Ok(result) if !parser.has_errors() => Some(result.ast.stmt().clone()),
            _ => None,
        }
    }

    fn enforce_user_management(
        &self,
        username: &str,
        stmt: &crate::query::parser::ast::Stmt,
        _space_id: i64,
    ) -> Result<(), GraphServiceError> {
        use crate::query::parser::ast::Stmt as S;
        let denied =
            |msg: String| GraphServiceError::new(format!("Permission check failed: {}", msg));
        match stmt {
            S::CreateUser(_) | S::AlterUser(_) | S::DropUser(_) => {
                if self.permission_manager.is_admin(username) {
                    Ok(())
                } else {
                    Err(denied("only Admin or God can manage users".to_string()))
                }
            }
            S::Grant(grant) => self.enforce_role_change(
                username,
                &grant.username,
                &grant.space_name,
                grant.role.as_str(),
            ),
            S::Revoke(revoke) => self.enforce_role_change(
                username,
                &revoke.username,
                &revoke.space_name,
                revoke.role.as_str(),
            ),
            S::ChangePassword(change) => {
                let target = change.username.as_deref().unwrap_or(username);
                if target == username || self.permission_manager.is_god(username) {
                    Ok(())
                } else {
                    Err(denied("can only change own password".to_string()))
                }
            }
            S::ShowUsers(_) | S::ShowRoles(_) => {
                if self.permission_manager.is_admin(username) {
                    Ok(())
                } else {
                    Err(denied(
                        "only Admin or God can list users and roles".to_string(),
                    ))
                }
            }
            S::DescribeUser(describe) => {
                if describe.username == username || self.permission_manager.is_admin(username) {
                    Ok(())
                } else {
                    Err(denied(
                        "only Admin or God can describe other users".to_string(),
                    ))
                }
            }
            _ => Err(denied("not an access-control statement".to_string())),
        }
    }

    fn enforce_role_change(
        &self,
        caller: &str,
        target_user: &str,
        space_name: &str,
        role_name: &str,
    ) -> Result<(), GraphServiceError> {
        let denied =
            |msg: String| GraphServiceError::new(format!("Permission check failed: {}", msg));
        if caller == target_user {
            return Err(denied("cannot modify own role".to_string()));
        }
        let target_role: linkrs_core::RoleType =
            role_name.parse().map_err(|e: String| denied(e))?;
        let space_id = self
            .storage
            .get_space_id(space_name)
            .map(|id| id as i64)
            .map_err(|e| denied(format!("space '{}' not found: {}", space_name, e)))?;
        let operator_role = self
            .permission_manager
            .get_role(caller, space_id)
            .or_else(|| {
                self.permission_manager
                    .get_role(caller, crate::permission::GOD_SPACE_ID)
            });
        let Some(op_role) = operator_role else {
            return Err(denied("only Admin or God can manage roles".to_string()));
        };
        if !matches!(
            op_role,
            linkrs_core::RoleType::God | linkrs_core::RoleType::Admin | linkrs_core::RoleType::Dba
        ) {
            return Err(denied("only Admin or God can manage roles".to_string()));
        }
        if !op_role.can_grant(target_role) {
            return Err(denied(format!("cannot grant role {}", target_role)));
        }
        Ok(())
    }

    /// Extract the SpaceSummary from a USE-statement result.
    ///
    /// The engine executes USE as a DataSet with `space_name`/`space_id`/
    /// `vid_type` columns; `QueryResult::space_summary` recognizes both
    /// representations.
    pub(crate) fn extract_space_summary_from_result(
        result: &linkrs_api::api_core::QueryResult,
    ) -> Option<SpaceSummary> {
        result.space_summary()
    }
}

use std::sync::Arc;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::spec::UserManageCommand;
use crate::storage::StorageAuthOps;
use linkrs_core::error::QueryError;
use linkrs_core::permission::RoleType;
use linkrs_core::types::user::{UserAlterInfo, UserInfo};
use linkrs_core::{NullType, Value};

pub(super) fn execute_user_manage(
    op: &mut super::DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let super::DdlOperatorKind::UserManage {
        storage,
        command,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    let result = match command {
        UserManageCommand::Create {
            username,
            password,
            if_not_exists,
        } => super::exec_auth(storage, |s| {
            if username.is_empty() {
                return Err(QueryError::execution(
                    "Username cannot be empty".to_string(),
                ));
            }
            if password.is_empty() {
                return Err(QueryError::execution(
                    "Password cannot be empty".to_string(),
                ));
            }
            let info = UserInfo::new(username.clone(), password.clone())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            let created = StorageAuthOps::create_user(s, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            if !created && !*if_not_exists {
                return Err(QueryError::execution(format!(
                    "User {} already exists",
                    username
                )));
            }
            Ok(())
        }),
        UserManageCommand::Drop {
            username,
            if_exists,
        } => super::exec_auth(storage, |s| {
            let dropped = StorageAuthOps::drop_user(s, username)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            if !dropped && !*if_exists {
                return Err(QueryError::execution(format!(
                    "User {} does not exist",
                    username
                )));
            }
            Ok(())
        }),
        UserManageCommand::Alter {
            username,
            new_password,
            is_locked,
            history_limit,
        } => super::exec_auth(storage, |s| {
            let mut alter_info = UserAlterInfo::new(username.clone());
            alter_info.history_limit = *history_limit;
            if let Some(password) = new_password {
                alter_info.new_password = Some(password.clone());
            }
            if let Some(locked) = is_locked {
                alter_info.is_locked = Some(*locked);
            }
            StorageAuthOps::alter_user(s, &alter_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageCommand::DescribeUser { username } => {
            let reader = super::get_reader(storage)?;
            let Some(info) = reader.get_user(username) else {
                return Err(QueryError::execution(format!(
                    "User {} does not exist",
                    username
                )));
            };
            let spaces = reader
                .list_spaces()
                .map_err(|e| QueryError::execution(e.to_string()))?;
            let mut assignments = reader.list_user_roles(username);
            assignments.sort_by_key(|(space_id, _)| *space_id);
            let schema = Arc::new(Schema::new(vec![
                ColumnInfo {
                    name: "username".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "locked".to_string(),
                    data_type: "bool".to_string(),
                },
                ColumnInfo {
                    name: "created_at".to_string(),
                    data_type: "bigint".to_string(),
                },
                ColumnInfo {
                    name: "last_login_at".to_string(),
                    data_type: "bigint".to_string(),
                },
                ColumnInfo {
                    name: "space_name".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "role".to_string(),
                    data_type: "string".to_string(),
                },
            ]));
            let base = vec![
                Value::string(info.username.clone()),
                Value::Bool(info.is_locked),
                Value::BigInt(info.created_at),
                info.last_login_at
                    .map(Value::BigInt)
                    .unwrap_or(Value::Null(NullType::Null)),
            ];
            let mut rows = Vec::with_capacity(assignments.len().max(1));
            if assignments.is_empty() {
                let mut row = base.clone();
                row.push(Value::string(String::new()));
                row.push(Value::string(String::new()));
                rows.push(row);
            } else {
                for (space_id, role) in assignments {
                    let mut row = base.clone();
                    row.push(Value::string(space_name_of(&spaces, space_id)));
                    row.push(Value::string(role.to_string()));
                    rows.push(row);
                }
            }
            Ok(Some(DataChunk::new(rows, schema)))
        }
        UserManageCommand::ChangePassword { password_info } => super::exec_auth(storage, |s| {
            StorageAuthOps::change_password(s, password_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageCommand::GrantRole {
            username,
            space_name,
            role,
        } => super::exec_auth(storage, |s| {
            let space_id = s
                .get_space_id(space_name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::grant_role(s, username, space_id as i64, *role)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageCommand::RevokeRole {
            username,
            space_name,
            role,
        } => super::exec_auth(storage, |s| {
            let space_id = s
                .get_space_id(space_name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            let actual = s
                .list_user_roles(username)
                .into_iter()
                .find(|(id, _)| *id == space_id as i64)
                .map(|(_, assigned)| assigned);
            match actual {
                None => {
                    return Err(QueryError::execution(format!(
                        "User {} has no role in space {}",
                        username, space_name
                    )));
                }
                Some(assigned) if assigned != *role => {
                    return Err(QueryError::execution(format!(
                        "User {} holds role {} in space {}, cannot revoke {}",
                        username, assigned, space_name, role
                    )));
                }
                Some(_) => {}
            }
            StorageAuthOps::revoke_role(s, username, space_id as i64)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageCommand::ShowUsers => {
            let reader = super::get_reader(storage)?;
            let users = reader.list_users();
            let rows: Vec<Vec<Value>> = users
                .iter()
                .map(|name| vec![Value::string(name.clone())])
                .collect();
            let schema = super::make_single_col_schema("user", "string");
            Ok(Some(DataChunk::new(rows, schema)))
        }
        UserManageCommand::ShowRoles { space_name } => {
            let reader = super::get_reader(storage)?;
            let spaces = reader
                .list_spaces()
                .map_err(|e| QueryError::execution(e.to_string()))?;
            let mut pairs: Vec<(String, i64, RoleType)> = reader
                .list_all_user_roles()
                .into_iter()
                .flat_map(|(username, roles)| {
                    roles
                        .into_iter()
                        .map(move |(space_id, role)| (username.clone(), space_id, role))
                })
                .collect();
            pairs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
            if !space_name.is_empty() {
                pairs.retain(|(_, space_id, _)| space_name_of(&spaces, *space_id) == *space_name);
            }
            let schema = Arc::new(Schema::new(vec![
                ColumnInfo {
                    name: "username".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "space_name".to_string(),
                    data_type: "string".to_string(),
                },
                ColumnInfo {
                    name: "role".to_string(),
                    data_type: "string".to_string(),
                },
            ]));
            let rows: Vec<Vec<Value>> = pairs
                .into_iter()
                .map(|(username, space_id, role)| {
                    vec![
                        Value::string(username),
                        Value::string(space_name_of(&spaces, space_id)),
                        Value::string(role.to_string()),
                    ]
                })
                .collect();
            Ok(Some(DataChunk::new(rows, schema)))
        }
    };
    result
}

/// Resolve a space id to its name via a `list_spaces` snapshot.
///
/// Mappings outliving their space (e.g. roles granted before a space drop)
/// render as the numeric id so stale assignments stay visible to auditors
/// instead of silently disappearing.
fn space_name_of(spaces: &[linkrs_core::types::SpaceInfo], space_id: i64) -> String {
    spaces
        .iter()
        .find(|info| info.space_id as i64 == space_id)
        .map(|info| info.space_name.clone())
        .unwrap_or_else(|| space_id.to_string())
}

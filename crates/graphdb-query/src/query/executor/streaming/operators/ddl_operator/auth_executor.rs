use crate::core::error::QueryError;
use crate::core::permission::RoleType;
use crate::core::types::user::{UserAlterInfo, UserInfo};
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::spec::UserManageCommand;
use crate::storage::StorageAuthOps;

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
            role,
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
            if !role.is_empty() && role.parse::<RoleType>().is_err() {
                return Err(QueryError::execution(format!(
                    "Unknown role type: {}",
                    role
                )));
            }
            let info = UserInfo::new(username.clone(), password.clone())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::create_user(s, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
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
            new_role,
            is_locked,
        } => super::exec_auth(storage, |s| {
            if let Some(role) = new_role {
                if role.parse::<RoleType>().is_err() {
                    return Err(QueryError::execution(format!(
                        "Unknown role type: {}",
                        role
                    )));
                }
            }
            let mut alter_info = UserAlterInfo::new(username.clone());
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
            if reader.user_exists(username) {
                let schema = super::make_single_col_schema("user", "string");
                Ok(Some(super::make_single_row(
                    schema,
                    vec![Value::string(format!("User '{}' exists", username))],
                )))
            } else {
                Err(QueryError::execution(format!(
                    "User {} does not exist",
                    username
                )))
            }
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
            let role = role.parse::<RoleType>().map_err(QueryError::execution)?;
            StorageAuthOps::grant_role(s, username, space_id, role)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageCommand::RevokeRole {
            username,
            space_name,
        } => super::exec_auth(storage, |s| {
            let space_id = s
                .get_space_id(space_name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::revoke_role(s, username, space_id)
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
        UserManageCommand::ShowRoles => {
            let schema = super::make_single_col_schema("role", "string");
            Ok(Some(DataChunk::new(Vec::new(), schema)))
        }
    };
    result
}

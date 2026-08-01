use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::permission::RoleType;
use crate::core::types::user::{UserAlterInfo, UserInfo};
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::planning::plan::core::nodes::management::manage_node_enums::UserManageNode;
use crate::storage::{QueryStorage, StorageAuthOps};

pub(super) fn execute_user_manage(
    storage: &Option<Arc<RwLock<dyn QueryStorage>>>,
    command: &UserManageNode,
    emitted: &mut bool,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    if *emitted {
        return Ok(None);
    }
    *emitted = true;
    if !base.lifecycle.is_opened() {
        return Ok(None);
    }
    let result = match command {
        UserManageNode::Create(node) => super::exec_auth(storage, |s| {
            if node.username().is_empty() {
                return Err(QueryError::execution(
                    "Username cannot be empty".to_string(),
                ));
            }
            if node.password().is_empty() {
                return Err(QueryError::execution(
                    "Password cannot be empty".to_string(),
                ));
            }
            if !node.role().is_empty()
                && node.role().parse::<RoleType>().is_err()
            {
                return Err(QueryError::execution(format!(
                    "Unknown role type: {}",
                    node.role()
                )));
            }
            let info = UserInfo::new(node.username().to_string(), node.password().to_string())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::create_user(s, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::Drop(node) => super::exec_auth(storage, |s| {
            let name = node.username();
            let dropped = StorageAuthOps::drop_user(s, name)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            if !dropped && !node.if_exists() {
                return Err(QueryError::execution(format!(
                    "User {} does not exist",
                    name
                )));
            }
            Ok(())
        }),
        UserManageNode::Alter(node) => super::exec_auth(storage, |s| {
            if let Some(role) = node.new_role() {
                if role.parse::<RoleType>().is_err() {
                    return Err(QueryError::execution(format!(
                        "Unknown role type: {}",
                        role
                    )));
                }
            }
            let mut alter_info = UserAlterInfo::new(node.username().to_string());
            if let Some(password) = node.new_password() {
                alter_info.new_password = Some(password.clone());
            }
            if let Some(is_locked) = node.is_locked() {
                alter_info.is_locked = Some(is_locked);
            }
            StorageAuthOps::alter_user(s, &alter_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::DescribeUser(node) => {
            let reader = super::get_reader(storage)?;
            let name = node.username();
            if reader.user_exists(name) {
                let schema = super::make_single_col_schema("user", "string");
                Ok(Some(super::make_single_row(
                    schema,
                    vec![Value::string(format!("User '{}' exists", name))],
                )))
            } else {
                Err(QueryError::execution(format!(
                    "User {} does not exist",
                    name
                )))
            }
        }
        UserManageNode::ChangePassword(node) => super::exec_auth(storage, |s| {
            let info = node.password_info();
            StorageAuthOps::change_password(s, info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::GrantRole(node) => super::exec_auth(storage, |s| {
            let space_id = s
                .get_space_id(node.space_name())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            let role = node
                .role()
                .parse::<RoleType>()
                .map_err(QueryError::execution)?;
            StorageAuthOps::grant_role(s, node.username(), space_id, role)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::RevokeRole(node) => super::exec_auth(storage, |s| {
            let space_id = s
                .get_space_id(node.space_name())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::revoke_role(s, node.username(), space_id)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::ShowUsers(_) => {
            let reader = super::get_reader(storage)?;
            let users = reader.list_users();
            let rows: Vec<Vec<Value>> = users
                .iter()
                .map(|name| vec![Value::string(name.clone())])
                .collect();
            let schema = super::make_single_col_schema("user", "string");
            Ok(Some(DataChunk::new(rows, schema)))
        }
        UserManageNode::ShowRoles(_) => {
            let schema = super::make_single_col_schema("role", "string");
            Ok(Some(DataChunk::new(Vec::new(), schema)))
        }
    };
    base.lifecycle.mark_closed();
    result
}

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::permission::RoleType;
use crate::core::types::user::{PasswordInfo, UserAlterInfo, UserInfo};
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
    let username = extract_user_manage_name(command);
    let result = match command {
        UserManageNode::Create(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("unknown");
            let info = UserInfo::new(name.to_string(), "".to_string())
                .map_err(|e| QueryError::execution(e.to_string()))?;
            StorageAuthOps::create_user(s, &info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::Drop(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("");
            StorageAuthOps::drop_user(s, name).map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::Alter(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("");
            let alter_info = UserAlterInfo::new(name.to_string());
            StorageAuthOps::alter_user(s, &alter_info)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::DescribeUser(_) => {
            let reader = super::get_reader(storage)?;
            let name = username.as_deref().unwrap_or("");
            let exists = reader.user_exists(name);
            if exists {
                let schema = super::make_single_col_schema("user", "string");
                Ok(Some(super::make_single_row(
                    schema,
                    vec![Value::String(format!("User '{}' exists", name))],
                )))
            } else {
                Ok(Some(super::make_manage_result(
                    "describe_user",
                    Some(name),
                    "not-found",
                )))
            }
        }
        UserManageNode::ChangePassword(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("");
            let pw = PasswordInfo {
                username: Some(name.to_string()),
                old_password: String::new(),
                new_password: String::new(),
            };
            StorageAuthOps::change_password(s, &pw)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::GrantRole(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("");
            StorageAuthOps::grant_role(s, name, 0, RoleType::User)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::RevokeRole(_) => super::exec_auth(storage, |s| {
            let name = username.as_deref().unwrap_or("");
            StorageAuthOps::revoke_role(s, name, 0)
                .map_err(|e| QueryError::execution(e.to_string()))?;
            Ok(())
        }),
        UserManageNode::ShowRoles(_) | UserManageNode::ShowUsers(_) => Err(QueryError::execution(
            "User listing is not exposed by StorageAuthOps".to_string(),
        )),
    };
    base.lifecycle.mark_closed();
    result
}

fn extract_user_manage_name(node: &UserManageNode) -> Option<String> {
    use UserManageNode::*;
    match node {
        Create(node) => Some(node.username().to_string()),
        Alter(node) => Some(node.username().to_string()),
        Drop(node) => Some(node.username().to_string()),
        DescribeUser(node) => Some(node.username().to_string()),
        GrantRole(node) => Some(node.username().to_string()),
        RevokeRole(node) => Some(node.username().to_string()),
        ChangePassword(_) | ShowRoles(_) | ShowUsers(_) => None,
    }
}

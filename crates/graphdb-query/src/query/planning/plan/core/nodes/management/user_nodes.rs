//! Implementation of the user management node
//!
//! Provide definitions for the planning nodes related to user management.

use crate::core::types::PasswordInfo;
use crate::define_plan_node;

define_plan_node! {
    pub struct CreateUserNode {
        username: String,
        password: String,
        role: String,
        if_not_exists: bool,
    }
    manage_enum: UserManageNode::Create as UserManage
    input: ZeroInputNode
}

impl CreateUserNode {
    pub fn new(id: i64, username: String, password: String) -> Self {
        Self {
            id,
            username,
            password,
            role: "user".to_string(),
            if_not_exists: false,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn with_role(mut self, role: String) -> Self {
        self.role = role;
        self
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> &str {
        &self.password
    }

    pub fn role(&self) -> &str {
        &self.role
    }

    pub fn if_not_exists(&self) -> bool {
        self.if_not_exists
    }
}

define_plan_node! {
    pub struct AlterUserNode {
        username: String,
        new_role: Option<String>,
        is_locked: Option<bool>,
    }
    manage_enum: UserManageNode::Alter as UserManage
    input: ZeroInputNode
}

impl AlterUserNode {
    pub fn new(id: i64, username: String) -> Self {
        Self {
            id,
            username,
            new_role: None,
            is_locked: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn with_role(mut self, role: String) -> Self {
        self.new_role = Some(role);
        self
    }

    pub fn with_locked(mut self, is_locked: bool) -> Self {
        self.is_locked = Some(is_locked);
        self
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn new_role(&self) -> Option<&String> {
        self.new_role.as_ref()
    }

    pub fn is_locked(&self) -> Option<bool> {
        self.is_locked
    }
}

define_plan_node! {
    pub struct DropUserNode {
        username: String,
        if_exists: bool,
    }
    manage_enum: UserManageNode::Drop as UserManage
    input: ZeroInputNode
}

impl DropUserNode {
    pub fn new(id: i64, username: String) -> Self {
        Self {
            id,
            username,
            output_var: None,
            col_names: Vec::new(),
            if_exists: false,
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

define_plan_node! {
    pub struct ChangePasswordNode {
        password_info: PasswordInfo,
    }
    manage_enum: UserManageNode::ChangePassword as UserManage
    input: ZeroInputNode
}

impl ChangePasswordNode {
    pub fn new(id: i64, password_info: PasswordInfo) -> Self {
        Self {
            id,
            password_info,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn password_info(&self) -> &PasswordInfo {
        &self.password_info
    }
}

define_plan_node! {
    pub struct GrantRoleNode {
        username: String,
        space_name: String,
        role: String,
    }
    manage_enum: UserManageNode::GrantRole as UserManage
    input: ZeroInputNode
}

impl GrantRoleNode {
    pub fn new(id: i64, username: String, space_name: String, role: String) -> Self {
        Self {
            id,
            username,
            space_name,
            role,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn role(&self) -> &str {
        &self.role
    }
}

define_plan_node! {
    pub struct RevokeRoleNode {
        username: String,
        space_name: String,
    }
    manage_enum: UserManageNode::RevokeRole as UserManage
    input: ZeroInputNode
}

impl RevokeRoleNode {
    pub fn new(id: i64, username: String, space_name: String) -> Self {
        Self {
            id,
            username,
            space_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

define_plan_node! {
    pub struct ShowUsersNode {}
    manage_enum: UserManageNode::ShowUsers as UserManage
    input: ZeroInputNode
}

impl ShowUsersNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
        }
    }
}

define_plan_node! {
    pub struct ShowRolesNode {
        space_name: String,
    }
    manage_enum: UserManageNode::ShowRoles as UserManage
    input: ZeroInputNode
}

impl ShowRolesNode {
    pub fn new(id: i64, space_name: String) -> Self {
        Self {
            id,
            space_name,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }
}

define_plan_node! {
    pub struct DescribeUserNode {
        username: String,
    }
    manage_enum: UserManageNode::DescribeUser as UserManage
    input: ZeroInputNode
}

impl DescribeUserNode {
    pub fn new(id: i64, username: String) -> Self {
        Self {
            id,
            username,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }
}

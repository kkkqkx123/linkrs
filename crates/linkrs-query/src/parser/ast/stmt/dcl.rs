use linkrs_core::permission::RoleType;
use linkrs_core::types::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserStmt {
    pub span: Span,
    pub username: String,
    pub password: String,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterUserStmt {
    pub span: Span,
    pub username: String,
    pub password: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DropUserStmt {
    pub span: Span,
    pub username: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangePasswordStmt {
    pub span: Span,
    pub username: Option<String>,
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GrantStmt {
    pub span: Span,
    pub role: RoleType,
    pub space_name: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RevokeStmt {
    pub span: Span,
    pub role: RoleType,
    pub space_name: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DescribeUserStmt {
    pub span: Span,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowUsersStmt {
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShowRolesStmt {
    pub span: Span,
    pub space_name: Option<String>,
}

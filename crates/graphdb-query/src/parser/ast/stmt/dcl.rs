use graphdb_core::types::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct CreateUserStmt {
    pub span: Span,
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlterUserStmt {
    pub span: Span,
    pub username: String,
    pub password: Option<String>,
    pub new_role: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleType {
    God,
    Admin,
    Dba,
    User,
    Guest,
}

impl RoleType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoleType::God => "GOD",
            RoleType::Admin => "ADMIN",
            RoleType::Dba => "DBA",
            RoleType::User => "USER",
            RoleType::Guest => "GUEST",
        }
    }
}

impl std::str::FromStr for RoleType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "GOD" => Ok(RoleType::God),
            "ADMIN" => Ok(RoleType::Admin),
            "DBA" => Ok(RoleType::Dba),
            "USER" => Ok(RoleType::User),
            "GUEST" => Ok(RoleType::Guest),
            _ => Err(format!("Unknown character type: {}", s)),
        }
    }
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

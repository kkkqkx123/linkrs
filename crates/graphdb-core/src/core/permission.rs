//! Permission Type Definition
//!
//! Provide core permission model and role type definitions

/// Permission Type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Read,
    Write,
    Delete,
    Schema,
    Admin,
}

/// 5-level permission model - reference nebula-graph implementation
/// - God: global super administrator with all privileges (similar to Linux root)
/// - Admin: Space administrator who can manage Schema and users in Space.
/// - Dba: Database administrator who can modify the Schema.
/// - User: Normal user, can read and write data
/// - Guest: read-only user, can only read data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleType {
    God = 0x01,
    Admin = 0x02,
    Dba = 0x03,
    User = 0x04,
    Guest = 0x05,
}

impl RoleType {
    /// Check that the role has the specified permissions
    pub fn has_permission(&self, permission: Permission) -> bool {
        match self {
            RoleType::God => true,
            RoleType::Admin => matches!(
                permission,
                Permission::Read
                    | Permission::Write
                    | Permission::Delete
                    | Permission::Schema
                    | Permission::Admin
            ),
            RoleType::Dba => matches!(
                permission,
                Permission::Read | Permission::Write | Permission::Delete | Permission::Schema
            ),
            RoleType::User => matches!(
                permission,
                Permission::Read | Permission::Write | Permission::Delete
            ),
            RoleType::Guest => matches!(permission, Permission::Read),
        }
    }

    /// Checks if the specified role can be granted
    pub fn can_grant(&self, target_role: RoleType) -> bool {
        match self {
            RoleType::God => target_role != RoleType::God,
            RoleType::Admin => matches!(
                target_role,
                RoleType::Dba | RoleType::User | RoleType::Guest
            ),
            RoleType::Dba => matches!(target_role, RoleType::User | RoleType::Guest),
            _ => false,
        }
    }

    /// Check if the specified role can be revoked
    pub fn can_revoke(&self, target_role: RoleType) -> bool {
        self.can_grant(target_role)
    }

    /// Parsing role types from bytes
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(RoleType::God),
            0x02 => Some(RoleType::Admin),
            0x03 => Some(RoleType::Dba),
            0x04 => Some(RoleType::User),
            0x05 => Some(RoleType::Guest),
            _ => None,
        }
    }

    /// convert to bytes
    pub fn to_byte(&self) -> u8 {
        *self as u8
    }
}

impl std::fmt::Display for RoleType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleType::God => write!(f, "GOD"),
            RoleType::Admin => write!(f, "ADMIN"),
            RoleType::Dba => write!(f, "DBA"),
            RoleType::User => write!(f, "USER"),
            RoleType::Guest => write!(f, "GUEST"),
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
            _ => Err(format!("Unknown role type: {}", s)),
        }
    }
}

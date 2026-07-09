//! User management actuator
//!
//! Provide user management function (support multi-user, 5-level permission model).

pub mod alter_user;
pub mod change_password;
pub mod create_user;
pub mod describe_user;
pub mod drop_user;
pub mod grant_role;
pub mod revoke_role;

pub use alter_user::AlterUserExecutor;
pub use change_password::ChangePasswordExecutor;
pub use create_user::CreateUserExecutor;
pub use describe_user::DescribeUserExecutor;
pub use drop_user::DropUserExecutor;
pub use grant_role::GrantRoleExecutor;
pub use revoke_role::RevokeRoleExecutor;

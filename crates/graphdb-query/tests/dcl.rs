//! Data Control Language (DCL) Integration Tests
//!
//! Test coverage:
//! - CREATE USER - Create a user
//! - ALTER USER - Modifies a user account
//! - DROP USER - Deletes a user
//! - CHANGE PASSWORD - Change your password
//! - GRANT - Grant privileges to users
//! - REVOKE - Revoke privileges from users
//! - SHOW USERS - List all users
//! - SHOW ROLES - List all roles
//! - DESCRIBE USER - Describe user details

#[path = "dcl/cascade_operations.rs"]
mod cascade_operations;
mod common;
#[path = "dcl/concurrent_operations.rs"]
mod concurrent_operations;
#[path = "dcl/cross_operation_consistency.rs"]
mod cross_operation_consistency;
#[path = "dcl/edge_cases.rs"]
mod edge_cases;
#[path = "dcl/permission.rs"]
mod permission;
#[path = "dcl/role.rs"]
mod role;
#[path = "dcl/security_tests.rs"]
mod security_tests;
#[path = "dcl/transaction_consistency.rs"]
mod transaction_consistency;
#[path = "dcl/user_management.rs"]
mod user_management;

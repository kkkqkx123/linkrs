//! Client Sessions Module
//!
//! Split the multiple responsibilities of the ClientSession into separate context modules:
//! - `session`: basic session information
//! - `space_context`: space context
//! - `role_context`: role context
//! - `query_context`: query context
//! - `transaction_context`: transaction context
//! - `statistics`: statistical information

pub mod client_session;
pub mod query_context;
pub mod role_context;
pub mod session;
pub mod space_context;
pub mod statistics;
pub mod transaction_context;

pub use client_session::ClientSession;
pub use query_context::QueryContext;
pub use role_context::RoleContext;
pub use session::Session;
pub use space_context::SpaceContext;
pub use transaction_context::TransactionContext;

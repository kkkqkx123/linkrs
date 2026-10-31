//! Authorization helpers.
//!
//! The implementation lives in [`crate::services::authz`]; this module keeps
//! the historical `http::handlers::authz` path available to HTTP handlers.

pub(crate) use crate::services::authz::{
    find_session, require_admin_session, require_schema_write_session,
    require_session_owner_or_admin, require_space_read, require_space_write,
};

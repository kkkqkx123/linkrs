//! Schema Extension Handlers
//!
//! Management view over schema objects, complementing the public
//! `http::handlers::schema` API: the public API serves external clients
//! while these handlers serve the web management console.
//!
//! The handlers live in submodules grouped by resource; this file only
//! declares the modules and re-exports the public surface so existing
//! paths such as `schema_ext::create_routes` keep working.
//!
//! - [`space`]: space listing, details and statistics.
//! - [`tag`]: tag listing, creation and management.
//! - [`edge_type`]: edge type listing, creation and management.
//! - [`index`]: index management and rebuild.
//! - [`routes`]: route assembly.
//! - [`common`]: shared parsing helpers.

pub mod common;
pub mod edge_type;
pub mod index;
pub mod routes;
pub mod space;
pub mod tag;

pub use edge_type::{
    create_edge_type, delete_edge_type, get_edge_type, list_edge_types, update_edge_type,
};
pub use index::{create_index, delete_index, get_index, list_indexes, rebuild_index};
pub use routes::create_routes;
pub use space::{get_space_details, get_space_statistics, list_spaces};
pub use tag::{create_tag, delete_tag, get_tag, list_tags, update_tag};

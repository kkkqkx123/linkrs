//! Public schema and migration HTTP handlers.
//!
//! The handlers live in submodules grouped by responsibility; this file
//! only declares the modules and re-exports the public surface so existing
//! paths such as `handlers::schema::create_space` keep working.
//!
//! - [`space`] / [`tag`] / [`edge_type`]: schema object management.
//! - [`version`]: version history, change ranges, breaking change detection.
//! - [`migration`]: migration plan generation, execution and tracking.
//! - [`common`]: shared parsing helpers.

pub mod common;
pub mod edge_type;
pub mod migration;
pub mod space;
pub mod tag;
pub mod version;

pub use edge_type::{create_edge_type, list_edge_types};
pub use migration::{
    create_migration_plan, dry_run_migration, execute_migration, migration_history,
    migration_status, rollback_migration,
};
pub use space::{create_space, drop_space, get_space, list_spaces};
pub use tag::{create_tag, list_tags};
pub use version::{detect_breaking_changes, get_schema_changes, get_version_history, ChangeInfo};

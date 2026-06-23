//! Schema change tracking and version management
//!
//! This module provides comprehensive schema change tracking and version management:
//!
//! - **Change Tracking**: Record every atomic schema modification (PropertyAdded, PropertyRemoved, etc.)
//! - **Change History**: Maintain per-label change logs indexed by version
//! - **Version Management**: Track schema versions and detect breaking changes between versions
//!
//! ## Components
//!
//! - `change`: Schema change events (`PropertyChange`, `ChangeDetails`) and change logs
//! - `version_history`: Version tracking for each label (`LabelVersionHistory`, `SchemaVersionHistory`)
//!
//! ## Current Capabilities
//!
//! - Record atomic schema changes via `PropertyChange` events
//! - Store changes in `ChangeLog` indexed by version number
//! - Query version history and detect breaking changes via `can_migrate()`
//!
//! ## Not Currently Supported
//!
//! - Automatic data migration on schema changes
//! - Zero-downtime schema upgrades
//! - Compatibility scoring and migration strategies
//!
//! If these features are needed in the future, refer to git history for the migration framework design.

pub mod change;
pub mod version_history;

pub use change::{ChangeDetails, ChangeLog, PropertyChange, SchemaObjectType};
pub use version_history::{
    LabelVersionHistory, SchemaVersionHistory,
};

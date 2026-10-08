//! Metadata Module
//!
//! Provides metadata context for the query planner.
//! Metadata is resolved directly by QueryPipelineManager from source components.

pub mod context;
pub mod types;

pub use context::MetadataContext;
pub use types::*;

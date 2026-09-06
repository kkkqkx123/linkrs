//! Conservative physical partition selection for streaming plans.
//!
//! The selector requires a **self-proven** vertex-id domain: the storage
//! layer observes every vertex-id write and can prove a covering
//! `[min, max]` range (see `StorageReader::vertex_id_domain`). Statistics can
//! estimate work, but cannot prove an ID range covers a scan; guessing a full
//! integer range would silently omit non-numeric or sparse identifiers.

pub mod config;
pub mod planner;
#[cfg(test)]
mod tests;

pub use config::{PartitioningConfig, PartitioningDecision, PartitioningLayoutInfo};
pub use planner::PartitioningPlanner;

//! Aggregate relevant rules
//!
//! These rules are responsible for optimizing aggregate operations.

pub mod push_filter_down_aggregate;

pub use push_filter_down_aggregate::PushFilterDownAggregateRule;

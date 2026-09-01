//! Storage Engine Module

pub mod background_freeze;
pub mod cache_manager;
pub mod config;
pub mod data_store;
pub mod freeze_decision;
pub mod gc_coordinator;
pub mod graph_storage;
pub(crate) mod params;
pub mod paths;
pub mod persistence_coordinator;
pub mod resource_budget;
pub mod snapshot_manager;
pub mod spiller;
pub mod sync_wrapper;
pub mod transaction;
pub mod wal_manager;

#[cfg(test)]
mod data_store_test;
#[cfg(test)]
mod persistence_test;

pub use params::{EdgeOperationParams, InsertEdgeParams};
pub use persistence_coordinator::PersistenceConfig;
pub use wal_manager::{WalManager, WalMetrics};

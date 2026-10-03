//! TransactionManager Tests
//!
//! Test transaction manager functionality, including transaction lifecycle management, concurrency control, timeout handling, etc.

use crate::manager::TransactionManager;
use crate::types::TransactionManagerConfig;

mod certify;
mod durability;
mod error;
mod lifecycle;
mod savepoint_owner_undo;
mod snapshot;

fn create_test_manager() -> TransactionManager {
    let config = TransactionManagerConfig {
        auto_cleanup: false,
        ..Default::default()
    };
    TransactionManager::new(config)
}

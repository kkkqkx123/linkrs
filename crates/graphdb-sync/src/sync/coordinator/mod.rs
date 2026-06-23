#![allow(clippy::module_inception)]

#[cfg(feature = "fulltext-search")]
pub mod coordinator;
pub mod error;
pub mod types;

#[cfg(feature = "fulltext-search")]
pub use coordinator::RecoveryResult;
#[cfg(feature = "fulltext-search")]
pub use coordinator::SyncCoordinator;
#[cfg(feature = "fulltext-search")]
pub use coordinator::SyncCoordinatorError;
pub use error::{CoordinatorError, CoordinatorResult, FulltextError, FulltextResult};
pub use types::{ChangeContext, ChangeData, ChangeType, IndexType};

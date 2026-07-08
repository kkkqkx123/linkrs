//! Fulltext Index Management Executor
//!
//! Provide functions for creating, deleting, altering, describing, and showing fulltext indexes.

#[cfg(feature = "fulltext-search")]
pub mod alter_fulltext_index;
#[cfg(feature = "fulltext-search")]
pub mod create_fulltext_index;
#[cfg(feature = "fulltext-search")]
pub mod describe_fulltext_index;
#[cfg(feature = "fulltext-search")]
pub mod drop_fulltext_index;
#[cfg(feature = "fulltext-search")]
pub mod show_fulltext_index;

#[cfg(all(test, feature = "fulltext-search"))]
mod tests;

#[cfg(feature = "fulltext-search")]
pub use alter_fulltext_index::AlterFulltextIndexExecutor;
#[cfg(feature = "fulltext-search")]
pub use create_fulltext_index::{CreateFulltextIndexConfig, CreateFulltextIndexExecutor};
#[cfg(feature = "fulltext-search")]
pub use describe_fulltext_index::DescribeFulltextIndexExecutor;
#[cfg(feature = "fulltext-search")]
pub use drop_fulltext_index::DropFulltextIndexExecutor;
#[cfg(feature = "fulltext-search")]
pub use show_fulltext_index::ShowFulltextIndexExecutor;

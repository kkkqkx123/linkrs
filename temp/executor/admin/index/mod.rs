//! Index Management Executor
//!
//! Provide functions for creating, deleting, describing, listing, reconstructing, and displaying the tag index and the edge index.

pub mod edge_index;
#[cfg(feature = "fulltext-search")]
pub mod fulltext_index;
pub mod rebuild_index;
pub mod show_tag_index_status;
pub mod tag_index;

#[cfg(test)]
mod tests;

pub use tag_index::{
    CreateTagIndexExecutor, DescTagIndexExecutor, DropTagIndexExecutor, ShowTagIndexesExecutor,
};

pub use edge_index::{
    CreateEdgeIndexExecutor, DescEdgeIndexExecutor, DropEdgeIndexExecutor, ShowEdgeIndexesExecutor,
};

pub use rebuild_index::{RebuildEdgeIndexExecutor, RebuildTagIndexExecutor};
pub use show_tag_index_status::ShowTagIndexStatusExecutor;

#[cfg(feature = "fulltext-search")]
pub use fulltext_index::{
    AlterFulltextIndexExecutor, CreateFulltextIndexConfig, CreateFulltextIndexExecutor,
    DescribeFulltextIndexExecutor, DropFulltextIndexExecutor, ShowFulltextIndexExecutor,
};

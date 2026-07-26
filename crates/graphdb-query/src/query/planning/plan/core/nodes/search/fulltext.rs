//! Full-Text Search Plan Nodes
//!
//! This module defines plan nodes for full-text search operations,
//! including data access and index management.

pub mod data_access;
pub mod management;

pub use data_access::{FulltextLookupNode, FulltextSearchNode, MatchFulltextNode};
pub use management::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, ShowFulltextIndexNode,
};

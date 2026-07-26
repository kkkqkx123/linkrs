//! Search Plan Nodes
//!
//! This module defines plan nodes for specialized search operations,
//! including full-text search and vector search.

pub mod fulltext;
pub mod vector;

pub use fulltext::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
    ShowFulltextIndexNode,
};
pub use vector::{CreateVectorIndexNode, CreateVectorIndexParams, DropVectorIndexNode};
#[cfg(feature = "qdrant")]
pub use vector::{
    OutputField, VectorLookupNode, VectorMatchNode, VectorSearchNode, VectorSearchParams,
};

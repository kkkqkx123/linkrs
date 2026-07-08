//! Vector Search Plan Nodes
//!
//! This module defines plan nodes for vector search operations,
//! including data access and index management.

#[cfg(feature = "qdrant")]
pub mod data_access;
pub mod management;

#[cfg(feature = "qdrant")]
pub use data_access::VectorSearchParams;
#[cfg(feature = "qdrant")]
pub use data_access::{OutputField, VectorLookupNode, VectorMatchNode, VectorSearchNode};
pub use management::{CreateVectorIndexNode, CreateVectorIndexParams, DropVectorIndexNode};

//! Local vector search engine.
//!
//! Tier 0 exact scan storage and search. Leaf crate: does not depend on any
//! graphdb crate and does not pull in the qdrant networking stack.

pub mod distance;
pub mod error;
pub mod filter;
pub mod storage;
pub mod types;

pub use error::{Result, VectorSearchError};
pub use types::*;

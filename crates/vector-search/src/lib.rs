//! Local vector search engine.
//!
//! Exact-scan storage and search. Leaf crate: does not depend on any
//! graphdb crate and does not pull in the qdrant networking stack.

pub mod distance;
pub mod engine;
pub mod error;
pub mod filter;
mod index;
pub mod storage;
pub mod types;

pub use engine::{LocalVectorEngine, TxnOp};
pub use error::{Result, VectorSearchError};
pub use types::*;

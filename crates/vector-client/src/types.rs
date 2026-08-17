//! Type forwarding.
//!
//! All shared vector types now live in `vector-search`. This module keeps the
//! old `vector_client::types` path working unchanged.

pub use vector_search::types;

pub use types::*;

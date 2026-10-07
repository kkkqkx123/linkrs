//! Type forwarding.
//!
//! All shared vector types now live in `simvec`. This module keeps the
//! old `vector_client::types` path working unchanged.

pub use simvec::types;

pub use types::*;

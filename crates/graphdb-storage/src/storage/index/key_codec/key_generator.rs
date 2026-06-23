//! Index Key Generator Trait
//!
//! This module defines the trait for abstracting key generation logic
//! for different index types.

/// Index key generator marker trait
///
/// Acts as a type-level marker to distinguish different index key generators
/// at the type level.
pub trait IndexKeyGenerator: Send + Sync + 'static {}

/// Vertex index key generator
pub struct VertexIndexKeyGen;

impl IndexKeyGenerator for VertexIndexKeyGen {}

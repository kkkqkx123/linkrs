//! Immutable configuration for vector search operators.

use crate::parser::ast::vector::VectorDistance;

/// Payload filter type for vector search.
///
/// Mirrors the planning-layer alias: the real `vector_search::VectorFilter`
/// when the vector feature is on, and an uninhabited-in-practice stub
/// otherwise (vector search itself is feature-gated).
#[cfg(feature = "vector")]
pub use vector_search::types::VectorFilter as SpecVectorFilter;

#[cfg(not(feature = "vector"))]
#[derive(Debug, Clone)]
pub struct SpecVectorFilter;

/// Vector index DDL command payload.
#[derive(Debug, Clone)]
pub enum VectorManageCommand {
    Create {
        index_name: String,
        tag_name: String,
        field_name: String,
        vector_size: usize,
        distance: VectorDistance,
        space_id: u64,
        hnsw_m: Option<usize>,
        hnsw_ef_construct: Option<usize>,
        quantization: Option<crate::parser::ast::vector::QuantizationKind>,
        quantile: Option<f32>,
        compression: Option<crate::parser::ast::vector::CompressionRatioKind>,
        always_ram: Option<bool>,
    },
    Drop {
        index_name: String,
        if_exists: bool,
        /// Pre-resolved coordinator location (space_id/tag/field); empty
        /// tag/field means the location could not be resolved at planning
        /// time and the executor reports a clear error or an `IF EXISTS`
        /// no-op status row.
        space_id: u64,
        tag_name: String,
        field_name: String,
    },
}

/// Immutable config for vector search operators.
#[derive(Debug, Clone)]
pub enum VectorSpec {
    VectorManage {
        space_name: String,
        command: VectorManageCommand,
    },
    VectorSearch {
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        /// Raw text for TEXT queries; resolved to a vector at execution time
        /// via the embedding service. `None` for Vector/Parameter queries
        /// where the vector is already resolved.
        query_text: Option<String>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        /// Minimum similarity score a result must reach.
        threshold: Option<f32>,
        /// Payload filter derived from the WHERE clause.
        filter: Option<SpecVectorFilter>,
        /// Number of leading result rows to skip after the engine returns
        /// `top_k + offset` candidates (OFFSET semantics).
        offset: usize,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<graphdb_core::types::ReadYourWritesConfig>,
    },
    VectorLookup {
        space_name: String,
        space_id: u64,
        index_name: String,
        query_vector: Vec<f32>,
        /// Raw text for TEXT queries; resolved to a vector at execution time.
        query_text: Option<String>,
        top_k: u32,
        tag_name: String,
        field_name: String,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<graphdb_core::types::ReadYourWritesConfig>,
    },
    VectorMatch {
        space_name: String,
        pattern: String,
        field: String,
        query_vector: Vec<f32>,
        /// Raw text for TEXT queries; resolved to a vector at execution time.
        query_text: Option<String>,
        threshold: Option<f32>,
        tag_name: String,
        field_name: String,
        space_id: u64,
        /// Read-your-writes consistency config; `None` = eventual.
        ryw_config: Option<graphdb_core::types::ReadYourWritesConfig>,
    },
}

impl VectorSpec {
    /// Return the RYW consistency config, if any.
    pub fn ryw_config(&self) -> Option<graphdb_core::types::ReadYourWritesConfig> {
        match self {
            Self::VectorManage { .. } => None,
            Self::VectorSearch { ryw_config, .. }
            | Self::VectorLookup { ryw_config, .. }
            | Self::VectorMatch { ryw_config, .. } => *ryw_config,
        }
    }
}

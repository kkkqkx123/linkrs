//! Metadata Context
//!
//! This module provides a context for storing and accessing metadata during query planning.
//! Similar to PostgreSQL's fdw_private mechanism.

use super::types::{EdgeTypeMetadata, IndexMetadata, TagMetadata};
use std::collections::HashMap;

/// Metadata context
///
/// Stores pre-resolved metadata during the planning phase, similar to PostgreSQL's fdw_private.
#[derive(Debug, Default, Clone)]
pub struct MetadataContext {
    /// Index metadata cache
    index_metadata: HashMap<String, IndexMetadata>,
    /// Tag metadata cache
    tag_metadata: HashMap<String, TagMetadata>,
    /// Edge type metadata cache
    edge_type_metadata: HashMap<String, EdgeTypeMetadata>,
}

impl MetadataContext {
    /// Create a new empty metadata context
    pub fn new() -> Self {
        Self {
            index_metadata: HashMap::new(),
            tag_metadata: HashMap::new(),
            edge_type_metadata: HashMap::new(),
        }
    }

    /// Set index metadata
    pub fn set_index_metadata(&mut self, index_name: String, metadata: IndexMetadata) {
        self.index_metadata.insert(index_name, metadata);
    }

    /// Set tag metadata
    pub fn set_tag_metadata(&mut self, tag_name: String, metadata: TagMetadata) {
        self.tag_metadata.insert(tag_name, metadata);
    }

    /// Set edge type metadata
    pub fn set_edge_type_metadata(&mut self, edge_type: String, metadata: EdgeTypeMetadata) {
        self.edge_type_metadata.insert(edge_type, metadata);
    }

    /// Get index metadata
    pub fn get_index_metadata(&self, index_name: &str) -> Option<&IndexMetadata> {
        self.index_metadata.get(index_name)
    }

    /// Get tag metadata
    pub fn get_tag_metadata(&self, tag_name: &str) -> Option<&TagMetadata> {
        self.tag_metadata.get(tag_name)
    }

    /// Get edge type metadata
    pub fn get_edge_type_metadata(&self, edge_type: &str) -> Option<&EdgeTypeMetadata> {
        self.edge_type_metadata.get(edge_type)
    }

    /// Check if index metadata exists
    pub fn has_index_metadata(&self, index_name: &str) -> bool {
        self.index_metadata.contains_key(index_name)
    }

    /// Check if tag metadata exists
    pub fn has_tag_metadata(&self, tag_name: &str) -> bool {
        self.tag_metadata.contains_key(tag_name)
    }

    /// Check if edge type metadata exists
    pub fn has_edge_type_metadata(&self, edge_type: &str) -> bool {
        self.edge_type_metadata.contains_key(edge_type)
    }

    /// Get all index metadata
    pub fn get_all_indexes(&self) -> impl Iterator<Item = &IndexMetadata> {
        self.index_metadata.values()
    }

    /// Get all tag metadata
    pub fn get_all_tags(&self) -> impl Iterator<Item = &TagMetadata> {
        self.tag_metadata.values()
    }

    /// Get all edge type metadata
    pub fn get_all_edge_types(&self) -> impl Iterator<Item = &EdgeTypeMetadata> {
        self.edge_type_metadata.values()
    }

    /// Clear all metadata
    pub fn clear(&mut self) {
        self.index_metadata.clear();
        self.tag_metadata.clear();
        self.edge_type_metadata.clear();
    }

    /// Merge another metadata context into this one
    pub fn merge(&mut self, other: MetadataContext) {
        self.index_metadata.extend(other.index_metadata);
        self.tag_metadata.extend(other.tag_metadata);
        self.edge_type_metadata.extend(other.edge_type_metadata);
    }

    /// Find a vector index by space_id and field name
    pub fn find_vector_index_by_field(
        &self,
        space_id: u64,
        field_name: &str,
    ) -> Option<&IndexMetadata> {
        self.index_metadata.values().find(|index| {
            index.space_id == space_id
                && index.field_name == field_name
                && index.index_type == super::types::IndexType::Vector
        })
    }
}

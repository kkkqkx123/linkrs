//! Storage Layer Testing Assistance Module
//!
//! Provide auxiliary functions for storage layer testing

#![allow(dead_code)]

use crate::core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo, TagInfo};
use crate::core::DataType;
use crate::storage::GraphStorage;
use parking_lot::{RwLock, RwLockWriteGuard};
use std::sync::Arc;

/// Create test image space information
pub fn create_test_space(name: &str) -> SpaceInfo {
    SpaceInfo::new(name.to_string())
        .with_vid_type(DataType::BigInt)
        .with_comment(Some("test_space".to_string()))
}

/// Create tag information
pub fn create_tag_info(name: &str, properties: Vec<(&str, DataType)>) -> TagInfo {
    let props = properties
        .into_iter()
        .map(|(name, data_type)| PropertyDef::new(name.to_string(), data_type))
        .collect();

    TagInfo::new(name.to_string()).with_properties(props)
}

/// Create edge type information.
pub fn create_edge_type_info(name: &str, properties: Vec<(&str, DataType)>) -> EdgeTypeInfo {
    let props = properties
        .into_iter()
        .map(|(name, data_type)| PropertyDef::new(name.to_string(), data_type))
        .collect();

    EdgeTypeInfo::new(name.to_string()).with_properties(props)
}

/// Create Person tag information (commonly used for testing purposes)
pub fn person_tag_info() -> TagInfo {
    create_tag_info(
        "Person",
        vec![("name", DataType::String), ("age", DataType::BigInt)],
    )
}

/// Create KNOWS edge type information (commonly used test edge types)
pub fn knows_edge_type_info() -> EdgeTypeInfo {
    create_edge_type_info("KNOWS", vec![("since", DataType::Date)])
}

/// Helper function to get storage guard from Arc<RwLock<GraphStorage>>
pub fn get_storage(storage: &Arc<RwLock<GraphStorage>>) -> RwLockWriteGuard<'_, GraphStorage> {
    storage.write()
}

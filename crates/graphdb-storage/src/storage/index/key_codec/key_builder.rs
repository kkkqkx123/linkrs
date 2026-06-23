//! Index Key Builder
//!
//! This module provides functions for building index keys.

use crate::core::{StorageError, Value};

use super::key_types::{
    serialize_value, ByteKey, KEY_TYPE_VERTEX_FORWARD, KEY_TYPE_VERTEX_REVERSE,
};

pub struct KeyBuilder;

impl KeyBuilder {
    // ========================================================================
    // Vertex Forward Index Keys
    // ========================================================================

    pub fn build_vertex_index_key(
        space_id: u64,
        index_name: &str,
        prop_value: &Value,
        vertex_id: &Value,
    ) -> Result<ByteKey, StorageError> {
        let prop_value_bytes = serialize_value(prop_value)?;
        let vertex_id_bytes = serialize_value(vertex_id)?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_FORWARD);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        key.extend_from_slice(&(prop_value_bytes.len() as u32).to_le_bytes());
        key.extend_from_slice(&prop_value_bytes);
        key.extend_from_slice(&(vertex_id_bytes.len() as u32).to_le_bytes());
        key.extend_from_slice(&vertex_id_bytes);

        Ok(ByteKey(key))
    }

    pub fn build_vertex_index_prefix(space_id: u64, index_name: &str) -> ByteKey {
        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_FORWARD);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        ByteKey(key)
    }

    // ========================================================================
    // Vertex Reverse Index Keys
    // ========================================================================

    pub fn build_vertex_reverse_key_v2(
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
    ) -> Result<ByteKey, StorageError> {
        let vertex_id_bytes = serialize_value(vertex_id)?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_REVERSE);
        key.extend_from_slice(&(vertex_id_bytes.len() as u32).to_le_bytes());
        key.extend_from_slice(&vertex_id_bytes);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());

        Ok(ByteKey(key))
    }

    pub fn build_vertex_reverse_prefix_v2(
        space_id: u64,
        vertex_id: &Value,
    ) -> Result<ByteKey, StorageError> {
        let vertex_id_bytes = serialize_value(vertex_id)?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_REVERSE);
        key.extend_from_slice(&(vertex_id_bytes.len() as u32).to_le_bytes());
        key.extend_from_slice(&vertex_id_bytes);

        Ok(ByteKey(key))
    }

    // ========================================================================
    // Range Query Helpers
    // ========================================================================

    pub fn build_range_end(prefix: &ByteKey) -> ByteKey {
        let mut end = prefix.0.clone();
        for i in (0..end.len()).rev() {
            if end[i] == 255 {
                end[i] = 0;
            } else {
                end[i] += 1;
                break;
            }
        }
        ByteKey(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    #[test]
    fn test_build_vertex_index_key() {
        let space_id = 1u64;
        let index_name = "idx_test";
        let prop_value = Value::String("test_value".to_string());
        let vertex_id = Value::Int(123);

        let key = KeyBuilder::build_vertex_index_key(space_id, index_name, &prop_value, &vertex_id)
            .expect("build_vertex_index_key should succeed");

        assert!(key.0.len() > 9);
        assert_eq!(key.0[8], KEY_TYPE_VERTEX_FORWARD);
    }

    #[test]
    fn test_build_vertex_reverse_key_v2() {
        let space_id = 1u64;
        let vertex_id = Value::Int(456);
        let index_name = "idx_test";

        let key = KeyBuilder::build_vertex_reverse_key_v2(space_id, &vertex_id, index_name)
            .expect("build_vertex_reverse_key_v2 should succeed");

        assert!(key.0.len() > 9);
        assert_eq!(key.0[8], KEY_TYPE_VERTEX_REVERSE);
    }

    #[test]
    fn test_build_range_end() {
        let prefix = ByteKey(vec![1, 2, 3]);
        let end = KeyBuilder::build_range_end(&prefix);
        assert_eq!(end.0, vec![1, 2, 4]);
    }

    #[test]
    fn test_build_range_end_overflow() {
        let prefix = ByteKey(vec![1, 255, 255]);
        let end = KeyBuilder::build_range_end(&prefix);
        assert_eq!(end.0, vec![2, 0, 0]);
    }
}

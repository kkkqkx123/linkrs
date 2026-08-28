//! Index Key Builder
//!
//! This module provides functions for building index keys using the
//! order-preserving `OrderedCodec`.

use graphdb_core::value::ordered_codec::OrderedCodec;
use graphdb_core::{StorageError, Value};

use super::key_types::{
    ByteKey, KEY_TYPE_EDGE_FORWARD, KEY_TYPE_EDGE_REVERSE, KEY_TYPE_VERTEX_FORWARD,
    KEY_TYPE_VERTEX_REVERSE,
};

pub struct KeyBuilder;

/// Shared codec instance for index key encoding.
pub fn codec() -> OrderedCodec {
    OrderedCodec::new()
}

/// Normalize integer values to the smallest type that fits, so that
/// `Value::BigInt(25)` and `Value::Int(25)` produce identical index key prefixes.
/// This ensures index keys built during INSERT/REBUILD and predicate prefixes
/// built during LOOKUP all use the same encoding for numerically equal values.
pub fn normalize_int_value(v: &Value) -> Value {
    match v {
        Value::BigInt(n) => {
            if *n >= i16::MIN as i64 && *n <= i16::MAX as i64 {
                return Value::SmallInt(*n as i16);
            }
            if *n >= i32::MIN as i64 && *n <= i32::MAX as i64 {
                return Value::Int(*n as i32);
            }
            v.clone()
        }
        Value::Int(n) => {
            if *n >= i16::MIN as i32 && *n <= i16::MAX as i32 {
                return Value::SmallInt(*n as i16);
            }
            v.clone()
        }
        _ => v.clone(),
    }
}

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
        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_FORWARD);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;
        key.extend_from_slice(&encoded_value);
        let encoded_entity = codec().encode(vertex_id)?;
        key.extend_from_slice(&encoded_entity);
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

    /// Build the prefix shared by all rows with one indexed property value.
    pub fn build_vertex_index_value_prefix(
        space_id: u64,
        index_name: &str,
        prop_value: &Value,
    ) -> Result<ByteKey, StorageError> {
        let mut key = Self::build_vertex_index_prefix(space_id, index_name).0;
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;
        key.extend_from_slice(&encoded_value);
        Ok(ByteKey(key))
    }

    // ========================================================================
    // Vertex Reverse Index Keys
    // ========================================================================

    pub fn build_vertex_reverse_key_v2(
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
    ) -> Result<ByteKey, StorageError> {
        let encoded_entity = codec().encode(vertex_id)?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_REVERSE);
        key.extend_from_slice(&encoded_entity);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());

        Ok(ByteKey(key))
    }

    /// Build a reverse key that includes the encoded property value,
    /// enabling forward-key reconstruction without a separate field.
    pub fn build_vertex_reverse_key_with_value(
        space_id: u64,
        vertex_id: &Value,
        index_name: &str,
        prop_value: &Value,
    ) -> Result<ByteKey, StorageError> {
        let encoded_entity = codec().encode(vertex_id)?;
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_VERTEX_REVERSE);
        key.extend_from_slice(&encoded_entity);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        key.extend_from_slice(&encoded_value);

        Ok(ByteKey(key))
    }

    // ========================================================================
    // Edge Forward Index Keys
    // ========================================================================

    pub fn build_edge_index_key(
        space_id: u64,
        index_name: &str,
        prop_value: &Value,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
    ) -> Result<ByteKey, StorageError> {
        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_EDGE_FORWARD);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;
        key.extend_from_slice(&encoded_value);
        let encoded_src = codec().encode(edge_src)?;
        key.extend_from_slice(&encoded_src);
        let encoded_dst = codec().encode(edge_dst)?;
        key.extend_from_slice(&encoded_dst);
        let encoded_type = codec().encode(&Value::string(edge_type))?;
        key.extend_from_slice(&encoded_type);
        let encoded_rank = codec().encode(&Value::BigInt(ranking))?;
        key.extend_from_slice(&encoded_rank);
        Ok(ByteKey(key))
    }

    pub fn build_edge_index_prefix(space_id: u64, index_name: &str) -> ByteKey {
        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_EDGE_FORWARD);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        ByteKey(key)
    }

    pub fn build_edge_index_value_prefix(
        space_id: u64,
        index_name: &str,
        prop_value: &Value,
    ) -> Result<ByteKey, StorageError> {
        let mut key = Self::build_edge_index_prefix(space_id, index_name).0;
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;
        key.extend_from_slice(&encoded_value);
        Ok(ByteKey(key))
    }

    // ========================================================================
    // Edge Reverse Index Keys
    // ========================================================================

    pub fn build_edge_reverse_key(
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
    ) -> Result<ByteKey, StorageError> {
        let encoded_src = codec().encode(edge_src)?;
        let encoded_dst = codec().encode(edge_dst)?;
        let encoded_type = codec().encode(&Value::string(edge_type))?;
        let encoded_rank = codec().encode(&Value::BigInt(ranking))?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_EDGE_REVERSE);
        key.extend_from_slice(&encoded_src);
        key.extend_from_slice(&encoded_dst);
        key.extend_from_slice(&encoded_type);
        key.extend_from_slice(&encoded_rank);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        Ok(ByteKey(key))
    }

    /// Build an edge reverse key that includes the encoded property value.
    pub fn build_edge_reverse_key_with_value(
        space_id: u64,
        edge_src: &Value,
        edge_dst: &Value,
        edge_type: &str,
        ranking: i64,
        index_name: &str,
        prop_value: &Value,
    ) -> Result<ByteKey, StorageError> {
        let encoded_src = codec().encode(edge_src)?;
        let encoded_dst = codec().encode(edge_dst)?;
        let encoded_type = codec().encode(&Value::string(edge_type))?;
        let encoded_rank = codec().encode(&Value::BigInt(ranking))?;
        let encoded_value = codec().encode(&normalize_int_value(prop_value))?;

        let mut key = Vec::new();
        key.extend_from_slice(&space_id.to_le_bytes());
        key.push(KEY_TYPE_EDGE_REVERSE);
        key.extend_from_slice(&encoded_src);
        key.extend_from_slice(&encoded_dst);
        key.extend_from_slice(&encoded_type);
        key.extend_from_slice(&encoded_rank);
        key.extend_from_slice(&(index_name.len() as u32).to_le_bytes());
        key.extend_from_slice(index_name.as_bytes());
        key.extend_from_slice(&encoded_value);
        Ok(ByteKey(key))
    }

    // ========================================================================
    // Range Query Helpers
    // ========================================================================

    /// Compute an exclusive upper bound for a prefix range.
    /// Delegates to [`OrderedCodec::prefix_upper_bound`].
    pub fn build_range_end(prefix: &ByteKey) -> ByteKey {
        ByteKey(OrderedCodec::prefix_upper_bound(&prefix.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

    #[test]
    fn test_build_vertex_index_key() {
        let space_id = 1u64;
        let index_name = "idx_test";
        let prop_value = Value::string("test_value");
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

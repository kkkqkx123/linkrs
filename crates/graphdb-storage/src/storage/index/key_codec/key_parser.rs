//! Index Key Parser
//!
//! This module provides functions for parsing index keys.

use crate::core::{StorageError, Value};

use super::key_types::deserialize_value;

pub struct KeyParser;

impl KeyParser {
    // ========================================================================
    // Vertex Forward Index Key Parsing
    // ========================================================================

    fn parse_key_parts(key_bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>, usize), StorageError> {
        let mut pos = 9;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error("Invalid key: too short".to_string()));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4 + index_name_len;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid key: missing prop_value_len".to_string(),
            ));
        }
        let prop_value_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + prop_value_len {
            return Err(StorageError::db_error(
                "Invalid key: prop_value exceeds key length".to_string(),
            ));
        }
        let prop_value = key_bytes[pos..pos + prop_value_len].to_vec();
        pos += prop_value_len;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid key: missing vertex_id_len".to_string(),
            ));
        }
        let vertex_id_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + vertex_id_len {
            return Err(StorageError::db_error(
                "Invalid key: vertex_id exceeds key length".to_string(),
            ));
        }
        let vertex_id = key_bytes[pos..pos + vertex_id_len].to_vec();

        Ok((prop_value, vertex_id, pos + vertex_id_len))
    }

    pub fn parse_vertex_id_from_key(key_bytes: &[u8]) -> Result<Value, StorageError> {
        let (_, vertex_id_bytes, _) = Self::parse_key_parts(key_bytes)?;
        deserialize_value(&vertex_id_bytes)
    }

    pub fn parse_prop_value_from_key(key_bytes: &[u8]) -> Result<Value, StorageError> {
        let (prop_value_bytes, _, _) = Self::parse_key_parts(key_bytes)?;
        deserialize_value(&prop_value_bytes)
    }

    // ========================================================================
    // Vertex Reverse Index Key Parsing
    // ========================================================================

    pub fn parse_vertex_reverse_key_v2(
        key_bytes: &[u8],
    ) -> Result<(Vec<u8>, String), StorageError> {
        if key_bytes.len() < 9 {
            return Err(StorageError::db_error(
                "Invalid reverse key v2: too short".to_string(),
            ));
        }

        let mut pos = 9;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid reverse key v2: missing vertex_id_len".to_string(),
            ));
        }
        let vertex_id_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + vertex_id_len {
            return Err(StorageError::db_error(
                "Invalid reverse key v2: vertex_id exceeds key length".to_string(),
            ));
        }
        let vertex_id_bytes = key_bytes[pos..pos + vertex_id_len].to_vec();
        pos += vertex_id_len;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid reverse key v2: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid reverse key v2: index_name exceeds key length".to_string(),
            ));
        }
        let index_name = String::from_utf8(key_bytes[pos..pos + index_name_len].to_vec())
            .map_err(|e| StorageError::db_error(format!("Invalid index_name encoding: {}", e)))?;

        Ok((vertex_id_bytes, index_name))
    }
}

#[cfg(test)]
mod tests {
    use super::super::key_types::serialize_value;
    use super::*;
    use crate::core::Value;
    use crate::storage::index::key_codec::key_builder::KeyBuilder;

    #[test]
    fn test_parse_vertex_id_from_key() {
        let space_id = 1u64;
        let index_name = "idx_test";
        let prop_value = Value::String("test_value".to_string());
        let vertex_id = Value::Int(123);

        let key = KeyBuilder::build_vertex_index_key(space_id, index_name, &prop_value, &vertex_id)
            .expect("build_vertex_index_key should succeed");

        let parsed_vid = KeyParser::parse_vertex_id_from_key(&key.0)
            .expect("parse_vertex_id_from_key should succeed");
        assert_eq!(parsed_vid, vertex_id);
    }

    #[test]
    fn test_parse_vertex_reverse_key_v2() {
        let space_id = 1u64;
        let vertex_id = Value::Int(456);
        let index_name = "idx_test";

        let key = KeyBuilder::build_vertex_reverse_key_v2(space_id, &vertex_id, index_name)
            .expect("build_vertex_reverse_key_v2 should succeed");

        let (parsed_vid_bytes, parsed_name) = KeyParser::parse_vertex_reverse_key_v2(&key.0)
            .expect("parse_vertex_reverse_key_v2 should succeed");
        assert_eq!(parsed_name, index_name);

        let vertex_id_bytes = serialize_value(&vertex_id).expect("serialize_value should succeed");
        assert_eq!(parsed_vid_bytes, vertex_id_bytes);
    }
}

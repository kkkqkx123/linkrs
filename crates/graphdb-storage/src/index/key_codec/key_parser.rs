//! Index Key Parser
//!
//! This module provides functions for parsing index keys encoded with the
//! `OrderedCodec`.

use graphdb_core::{StorageError, Value};

use super::key_builder::codec;

type EdgeReverseKeyParts = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, String);

pub struct KeyParser;

impl KeyParser {
    // ========================================================================
    // Vertex Forward Index Key Parsing
    // ========================================================================

    pub fn parse_vertex_id_from_key(key_bytes: &[u8]) -> Result<Value, StorageError> {
        let mut pos = 9;
        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error("Invalid key: too short".to_string()));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4 + index_name_len;

        // Skip the prop value
        let (_prop_value, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
        pos += consumed;

        // Decode the vertex ID
        let (vertex_id, _consumed2) = codec().decode_value_inner(&key_bytes[pos..])?;
        Ok(vertex_id)
    }

    // ========================================================================
    // Edge Forward Index Key Parsing
    // ========================================================================

    pub fn parse_edge_identity_from_key(
        key_bytes: &[u8],
    ) -> Result<(Value, Value, String, i64), StorageError> {
        let mut pos = 9;
        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid edge key: too short".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4 + index_name_len;

        let (_prop_value, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
        pos += consumed;

        let (src_val, consumed2) = codec().decode_value_inner(&key_bytes[pos..])?;
        pos += consumed2;

        let (dst_val, consumed3) = codec().decode_value_inner(&key_bytes[pos..])?;
        pos += consumed3;

        let (type_val, consumed4) = codec().decode_value_inner(&key_bytes[pos..])?;
        let edge_type = match type_val {
            Value::String(s) => s,
            _ => {
                return Err(StorageError::db_error(
                    "Invalid edge type encoding".to_string(),
                ));
            }
        };
        pos += consumed4;

        let (rank_val, _consumed5) = codec().decode_value_inner(&key_bytes[pos..])?;
        let ranking = match rank_val {
            Value::BigInt(v) => v,
            Value::Int(v) => v as i64,
            _ => {
                return Err(StorageError::db_error(
                    "Invalid ranking encoding".to_string(),
                ));
            }
        };

        Ok((src_val, dst_val, edge_type.to_string(), ranking))
    }

    // ========================================================================
    // Edge Reverse Index Key Parsing
    // ========================================================================

    pub fn parse_edge_reverse_key(key_bytes: &[u8]) -> Result<EdgeReverseKeyParts, StorageError> {
        if key_bytes.len() < 9 {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: too short".to_string(),
            ));
        }
        let mut pos = 9;

        let (src_val, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
        let src_bytes = codec().encode(&src_val)?;
        pos += consumed;

        let (dst_val, consumed2) = codec().decode_value_inner(&key_bytes[pos..])?;
        let dst_bytes = codec().encode(&dst_val)?;
        pos += consumed2;

        let (type_val, consumed3) = codec().decode_value_inner(&key_bytes[pos..])?;
        let type_bytes = codec().encode(&type_val)?;
        pos += consumed3;

        let (rank_val, consumed4) = codec().decode_value_inner(&key_bytes[pos..])?;
        let rank_bytes = codec().encode(&rank_val)?;
        pos += consumed4;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: index_name exceeds key length".to_string(),
            ));
        }
        let index_name = String::from_utf8(key_bytes[pos..pos + index_name_len].to_vec())
            .map_err(|e| StorageError::db_error(format!("Invalid index_name encoding: {}", e)))?;

        Ok((src_bytes, dst_bytes, type_bytes, rank_bytes, index_name))
    }

    // ========================================================================
    // Vertex Reverse Index Key Parsing
    // ========================================================================

    /// Parse a vertex reverse key (v2 format) and return the encoded vertex ID
    /// bytes and the index name.
    ///
    /// Format: `[space_id(8) LE] [KEY_TYPE_VERTEX_REVERSE(1)]
    ///         [OrderedCodec(vertex_id)] [index_name_len(4) LE] [index_name(N)]`
    pub fn parse_vertex_reverse_key_v2(
        key_bytes: &[u8],
    ) -> Result<(Vec<u8>, String), StorageError> {
        if key_bytes.len() < 9 {
            return Err(StorageError::db_error(
                "Invalid vertex reverse key: too short".to_string(),
            ));
        }
        let mut pos = 9;

        let (vertex_id, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
        let vertex_id_bytes = codec().encode(&vertex_id)?;
        pos += consumed;

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid vertex reverse key: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid vertex reverse key: index_name exceeds key length".to_string(),
            ));
        }
        let index_name = String::from_utf8(key_bytes[pos..pos + index_name_len].to_vec())
            .map_err(|e| StorageError::db_error(format!("Invalid index_name encoding: {}", e)))?;

        Ok((vertex_id_bytes, index_name))
    }

    /// Extract the encoded property value from a reverse key that includes it.
    /// Returns the remaining bytes after the index_name as the encoded value.
    pub fn extract_value_from_reverse_key(key_bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
        if key_bytes.len() < 9 {
            return Err(StorageError::db_error(
                "Invalid reverse key: too short".to_string(),
            ));
        }
        let mut pos = 9;
        let (_entity_id, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
        pos += consumed;
        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid reverse key: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;
        if key_bytes.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid reverse key: index_name exceeds key length".to_string(),
            ));
        }
        pos += index_name_len;
        Ok(key_bytes[pos..].to_vec())
    }

    /// Extract the encoded property value from a reverse key SUFFIX (without the leading
    /// space_id + key_type prefix). The suffix format is:
    ///   [OrderedCodec(entity_id)] [index_name_len(4) LE] [index_name(N)]
    ///   [OrderedCodec(prop_value)]
    pub fn extract_value_from_reverse_suffix(suffix: &[u8]) -> Result<Vec<u8>, StorageError> {
        if suffix.is_empty() {
            return Err(StorageError::db_error(
                "Invalid reverse key suffix: empty".to_string(),
            ));
        }
        let mut pos = 0;
        let (_entity_id, consumed) = codec().decode_value_inner(&suffix[pos..])?;
        pos += consumed;

        if suffix.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid reverse key suffix: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(suffix[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;
        if suffix.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid reverse key suffix: index_name exceeds suffix length".to_string(),
            ));
        }
        pos += index_name_len;
        Ok(suffix[pos..].to_vec())
    }

    /// Extract the encoded property value from an edge reverse key SUFFIX (without the leading
    /// space_id + key_type prefix). The suffix format is:
    ///   [OrderedCodec(src)] [OrderedCodec(dst)] [OrderedCodec(type)] [OrderedCodec(ranking)]
    ///   [index_name_len(4) LE] [index_name(N)] [OrderedCodec(prop_value)]
    pub fn extract_value_from_edge_reverse_suffix(suffix: &[u8]) -> Result<Vec<u8>, StorageError> {
        if suffix.is_empty() {
            return Err(StorageError::db_error(
                "Invalid edge reverse key suffix: empty".to_string(),
            ));
        }
        let mut pos = 0;
        for _ in 0..4 {
            let (_, consumed) = codec().decode_value_inner(&suffix[pos..])?;
            pos += consumed;
        }
        if suffix.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid edge reverse key suffix: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(suffix[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;
        if suffix.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid edge reverse key suffix: index_name exceeds suffix length".to_string(),
            ));
        }
        pos += index_name_len;
        Ok(suffix[pos..].to_vec())
    }

    /// Extract the encoded property value from an edge reverse key that includes it.
    pub fn extract_value_from_edge_reverse_key(key_bytes: &[u8]) -> Result<Vec<u8>, StorageError> {
        if key_bytes.len() < 9 {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: too short".to_string(),
            ));
        }
        let mut pos = 9;

        for _ in 0..4 {
            let (_, consumed) = codec().decode_value_inner(&key_bytes[pos..])?;
            pos += consumed;
        }

        if key_bytes.len() < pos + 4 {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: missing index_name_len".to_string(),
            ));
        }
        let index_name_len =
            u32::from_le_bytes(key_bytes[pos..pos + 4].try_into().unwrap_or([0; 4])) as usize;
        pos += 4;

        if key_bytes.len() < pos + index_name_len {
            return Err(StorageError::db_error(
                "Invalid edge reverse key: index_name exceeds key length".to_string(),
            ));
        }
        pos += index_name_len;

        Ok(key_bytes[pos..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;
    use crate::index::key_codec::key_builder::KeyBuilder;

    #[test]
    fn test_parse_vertex_id_from_key() {
        let space_id = 1u64;
        let index_name = "idx_test";
        let prop_value = Value::string("test_value");
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
        assert!(!parsed_vid_bytes.is_empty());
    }
}

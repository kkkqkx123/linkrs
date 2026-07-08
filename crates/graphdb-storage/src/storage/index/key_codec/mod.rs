//! Index Key Codec Module
//!
//! This module provides key encoding and decoding functionality
//! for the index system.
//!
//! ## Module Structure
//!
//! - `key_types`: Core types and constants for index keys
//! - `key_builder`: Functions for building index keys
//! - `key_parser`: Functions for parsing index keys
//! - `compression`: removed; index compression is not wired in this crate
//!
//! ## Usage
//!
//! ```rust,ignore
//! use graphdb::storage::index::key_codec::{KeyBuilder, KeyParser, ByteKey};
//!
//! // Build a vertex index key
//! let key = KeyBuilder::build_vertex_index_key(
//!     space_id,
//!     "idx_name",
//!     &prop_value,
//!     &vertex_id,
//! )?;
//!
//! // Parse the vertex ID from the key
//! let vertex_id = KeyParser::parse_vertex_id_from_key(&key.0)?;
//! ```

pub mod key_builder;
pub mod key_generator;
pub mod key_parser;
pub mod key_types;

pub use key_builder::KeyBuilder;
pub use key_generator::{IndexKeyGenerator, VertexIndexKeyGen};
pub use key_parser::KeyParser;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::storage::index::key_codec::key_types::{
        serialize_value, ByteKey, KEY_TYPE_VERTEX_FORWARD, KEY_TYPE_VERTEX_REVERSE,
    };

    #[test]
    fn test_build_and_parse_vertex_key() {
        let space_id = 1u64;
        let index_name = "idx_test";
        let prop_value = Value::String("test_value".to_string());
        let vertex_id = Value::Int(123);

        let key = KeyBuilder::build_vertex_index_key(space_id, index_name, &prop_value, &vertex_id)
            .expect("build_vertex_index_key should succeed");

        assert!(key.0.len() > 9);
        assert_eq!(key.0[8], KEY_TYPE_VERTEX_FORWARD);

        let parsed_vid = KeyParser::parse_vertex_id_from_key(&key.0)
            .expect("parse_vertex_id_from_key should succeed");
        assert_eq!(parsed_vid, vertex_id);
    }

    #[test]
    fn test_build_and_parse_vertex_reverse_key_v2() {
        let space_id = 1u64;
        let vertex_id = Value::Int(456);
        let index_name = "idx_test";

        let key = KeyBuilder::build_vertex_reverse_key_v2(space_id, &vertex_id, index_name)
            .expect("build_vertex_reverse_key_v2 should succeed");

        assert!(key.0.len() > 9);
        assert_eq!(key.0[8], KEY_TYPE_VERTEX_REVERSE);

        let (parsed_vid_bytes, parsed_name) = KeyParser::parse_vertex_reverse_key_v2(&key.0)
            .expect("parse_vertex_reverse_key_v2 should succeed");
        assert_eq!(parsed_name, index_name);

        let vertex_id_bytes = serialize_value(&vertex_id).expect("serialize_value should succeed");
        assert_eq!(parsed_vid_bytes, vertex_id_bytes);
    }

    #[test]
    fn test_build_range_end() {
        let prefix = ByteKey(vec![1, 2, 3]);
        let end = KeyBuilder::build_range_end(&prefix);
        assert_eq!(end.0, vec![1, 2, 4]);
    }
}

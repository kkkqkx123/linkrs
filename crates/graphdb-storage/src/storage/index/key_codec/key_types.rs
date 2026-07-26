//! Index Key Types and Constants
//!
//! This module defines the core types and constants used for index key encoding.
//! Value serialization now uses `OrderedCodec` from `graphdb-core` (order-preserving).

use crate::core::value::ordered_codec::OrderedCodec;
use crate::core::{StorageError, Value};

/// Byte key wrapper for index keys
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ByteKey(pub Vec<u8>);

impl AsRef<[u8]> for ByteKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for ByteKey {
    fn from(v: Vec<u8>) -> Self {
        ByteKey(v)
    }
}

impl From<ByteKey> for Vec<u8> {
    fn from(key: ByteKey) -> Self {
        key.0
    }
}

pub type SecondaryIndexKey = Vec<u8>;

pub const KEY_TYPE_VERTEX_REVERSE: u8 = 0x01;
pub const KEY_TYPE_VERTEX_FORWARD: u8 = 0x03;
pub const KEY_TYPE_EDGE_REVERSE: u8 = 0x02;
pub const KEY_TYPE_EDGE_FORWARD: u8 = 0x04;

/// Encode a Value using the order-preserving OrderedCodec.
///
/// Replaces the old postcard-based serialization which did not
/// preserve byte-order comparison.
pub fn serialize_value(value: &Value) -> Result<Vec<u8>, StorageError> {
    OrderedCodec::new().encode(value)
}

/// Decode a Value from OrderedCodec bytes.
pub fn deserialize_value(data: &[u8]) -> Result<Value, StorageError> {
    OrderedCodec::new().decode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_value() {
        let value = Value::string("test");
        let bytes = serialize_value(&value).expect("serialize_value should succeed");
        let decoded = deserialize_value(&bytes).expect("deserialize_value should succeed");
        assert_eq!(value, decoded);
    }

    #[test]
    fn test_byte_key_from_vec() {
        let vec = vec![1, 2, 3, 4];
        let key: ByteKey = vec.clone().into();
        assert_eq!(key.0, vec);
    }

    #[test]
    fn test_byte_key_as_ref() {
        let key = ByteKey(vec![1, 2, 3]);
        assert_eq!(key.as_ref(), &[1, 2, 3]);
    }
}

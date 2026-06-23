//! Index Key Types and Constants
//!
//! This module defines the core types and constants used for index key encoding.

use crate::core::{StorageError, Value};
use postcard::{from_bytes, to_allocvec};

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

pub fn serialize_value(value: &Value) -> Result<Vec<u8>, StorageError> {
    to_allocvec(value).map_err(|e| StorageError::serialize_error(e.to_string()))
}

pub fn deserialize_value(data: &[u8]) -> Result<Value, StorageError> {
    from_bytes(data).map_err(|e| StorageError::deserialize_error(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_deserialize_value() {
        let value = Value::String("test".to_string());
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

//! Index Key Types and Constants
//!
//! This module defines the core types and constants used for index key encoding.
//! Value serialization now uses `OrderedCodec` from `graphdb-core` (order-preserving).

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

#[cfg(test)]
mod tests {
    use super::*;

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

//! Variable-Length Integer (Varint) Encoding
//!
//! Provides space-efficient encoding for unsigned integers.
//! Small integers (< 128) use 1 byte; larger values use 2-5 bytes with continuation bits.
//!
//! Encoding format: Each byte uses bit 7 as a continuation flag (1 = more bytes follow),
//! and bits 0-6 store 7 bits of the value.

use std::io::{self, Read, Write};

use crate::core::StorageResult;

/// Encode a u32 value as variable-length integer
///
/// # Returns
/// A Vec containing the encoded bytes
///
/// # Example
/// ```ignore
/// let encoded = encode_varint(127);      // 1 byte: [127]
/// let encoded = encode_varint(128);      // 2 bytes: [0x80, 0x01]
/// ```
pub fn encode_varint(mut value: u32) -> Vec<u8> {
    let mut result = Vec::new();

    while value >= 128 {
        result.push(((value as u8) & 0x7F) | 0x80);
        value >>= 7;
    }

    result.push(value as u8);
    result
}

/// Decode a variable-length integer from a byte slice
///
/// # Arguments
/// * `bytes` - The byte slice to read from
/// * `offset` - Mutable reference to current offset in the slice
///
/// # Returns
/// The decoded u32 value
///
/// # Errors
/// Returns error if EOF is reached before a complete varint is read
pub fn decode_varint(bytes: &[u8], offset: &mut usize) -> StorageResult<u32> {
    let mut result = 0u32;
    let mut shift = 0;

    loop {
        if *offset >= bytes.len() {
            return Err(crate::core::StorageError::io_error(
                "unexpected EOF while decoding varint".to_string(),
            ));
        }

        let byte = bytes[*offset];
        *offset += 1;

        result |= ((byte & 0x7F) as u32) << shift;

        if byte < 128 {
            break;
        }

        shift += 7;

        if shift >= 32 {
            return Err(crate::core::StorageError::io_error(
                "varint overflow: too many continuation bytes".to_string(),
            ));
        }
    }

    Ok(result)
}

/// Decode a variable-length integer from a reader
///
/// # Arguments
/// * `reader` - The reader to read from
///
/// # Returns
/// The decoded u32 value
///
/// # Errors
/// Returns error if IO fails or varint is malformed
pub fn decode_varint_reader<R: Read>(reader: &mut R) -> StorageResult<u32> {
    let mut result = 0u32;
    let mut shift = 0;
    let mut buf = [0u8; 1];

    loop {
        reader.read_exact(&mut buf).map_err(|e| {
            crate::core::StorageError::io_error(format!("failed to read varint byte: {}", e))
        })?;

        let byte = buf[0];
        result |= ((byte & 0x7F) as u32) << shift;

        if byte < 128 {
            break;
        }

        shift += 7;

        if shift >= 32 {
            return Err(crate::core::StorageError::io_error(
                "varint overflow: too many continuation bytes".to_string(),
            ));
        }
    }

    Ok(result)
}

/// Calculate the encoded length of a varint for the given value
///
/// # Example
/// ```ignore
/// assert_eq!(varint_len(127), 1);
/// assert_eq!(varint_len(128), 2);
/// assert_eq!(varint_len(16_383), 2);
/// assert_eq!(varint_len(16_384), 3);
/// ```
pub fn varint_len(mut value: u32) -> usize {
    if value == 0 {
        return 1;
    }

    let mut len = 0;
    while value > 0 {
        len += 1;
        value >>= 7;
    }
    len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_encode_decode_small() {
        let values = vec![0, 1, 127];

        for val in values {
            let encoded = encode_varint(val);
            assert_eq!(encoded.len(), 1, "small value {} should encode to 1 byte", val);

            let mut offset = 0;
            let decoded = decode_varint(&encoded, &mut offset).unwrap();
            assert_eq!(decoded, val, "roundtrip failed for {}", val);
            assert_eq!(offset, encoded.len());
        }
    }

    #[test]
    fn test_varint_encode_decode_medium() {
        let values = vec![128, 16_383, 16_384, 2_097_151, 2_097_152];

        for val in values {
            let encoded = encode_varint(val);
            let mut offset = 0;
            let decoded = decode_varint(&encoded, &mut offset).unwrap();
            assert_eq!(decoded, val, "roundtrip failed for {}", val);
            assert_eq!(offset, encoded.len());
        }
    }

    #[test]
    fn test_varint_encode_decode_large() {
        let values = vec![u32::MAX / 2, u32::MAX - 1, u32::MAX];

        for val in values {
            let encoded = encode_varint(val);
            let mut offset = 0;
            let decoded = decode_varint(&encoded, &mut offset).unwrap();
            assert_eq!(decoded, val, "roundtrip failed for {}", val);
            assert_eq!(offset, encoded.len());
        }
    }

    #[test]
    fn test_varint_encode_specific() {
        // Test specific encoding formats
        assert_eq!(encode_varint(0), vec![0]);
        assert_eq!(encode_varint(127), vec![127]);
        assert_eq!(encode_varint(128), vec![0x80, 0x01]);
        assert_eq!(encode_varint(16_384), vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn test_varint_decode_error_eof() {
        let bytes = vec![0x80]; // Incomplete varint (missing continuation byte)

        let mut offset = 0;
        let result = decode_varint(&bytes, &mut offset);

        assert!(result.is_err(), "should fail on incomplete varint");
    }

    #[test]
    fn test_varint_decode_error_overflow() {
        // Create a varint with too many continuation bits
        let bytes = vec![0x80, 0x80, 0x80, 0x80, 0x80];

        let mut offset = 0;
        let result = decode_varint(&bytes, &mut offset);

        assert!(result.is_err(), "should fail on overflow");
    }

    #[test]
    fn test_varint_len() {
        assert_eq!(varint_len(0), 1);
        assert_eq!(varint_len(127), 1);
        assert_eq!(varint_len(128), 2);
        assert_eq!(varint_len(16_383), 2);
        assert_eq!(varint_len(16_384), 3);
        assert_eq!(varint_len(2_097_151), 3);
        assert_eq!(varint_len(2_097_152), 4);
        assert_eq!(varint_len(u32::MAX), 5);
    }

    #[test]
    fn test_varint_reader() {
        let values = vec![0, 127, 128, 16_384, u32::MAX];

        for val in values {
            let encoded = encode_varint(val);
            let mut cursor = std::io::Cursor::new(encoded);
            let decoded = decode_varint_reader(&mut cursor).unwrap();
            assert_eq!(decoded, val, "reader roundtrip failed for {}", val);
        }
    }

    #[test]
    fn test_varint_reader_error_eof() {
        let bytes = vec![0x80];
        let mut cursor = std::io::Cursor::new(bytes);
        let result = decode_varint_reader(&mut cursor);

        assert!(result.is_err(), "should fail on incomplete varint");
    }
}

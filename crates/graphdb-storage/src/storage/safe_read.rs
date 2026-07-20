//! Safe bounded reader for binary deserialization.
//!
//! Provides a `BoundedReader` wrapper that limits how many bytes can be read
//! from an underlying reader, preventing memory exhaustion from malicious or
//! corrupted length fields. Also defines the `SafeSerializable` trait used
//! by on-disk format types to perform self-describing serialization with
//! inherent bounds checking.

use std::io::Read;

use crate::core::{StorageError, StorageResult};

/// A reader wrapper that enforces a byte limit on all reads.
///
/// When deserializing from untrusted sources (files on disk, network),
/// length fields are used to allocate buffers. Without a bound, a corrupted
/// length field could cause multi-gigabyte allocations. `BoundedReader`
/// tracks remaining bytes and rejects reads that exceed the limit.
pub struct BoundedReader<'a> {
    inner: &'a mut dyn Read,
    remaining: usize,
}

impl<'a> BoundedReader<'a> {
    /// Create a new bounded reader with the given byte limit.
    pub fn new(inner: &'a mut impl Read, limit: usize) -> Self {
        Self {
            inner: inner as &mut dyn Read,
            remaining: limit,
        }
    }

    /// Read exactly `buf.len()` bytes, failing if it would exceed the remaining limit.
    pub fn read_exact(&mut self, buf: &mut [u8]) -> StorageResult<()> {
        if buf.len() > self.remaining {
            return Err(StorageError::deserialize_error(format!(
                "read of {} bytes exceeds remaining bound of {}",
                buf.len(),
                self.remaining
            )));
        }
        self.inner.read_exact(buf)?;
        self.remaining -= buf.len();
        Ok(())
    }

    /// Skip all remaining bounded bytes.
    pub fn skip_all(&mut self) -> StorageResult<()> {
        if self.remaining > 0 {
            let copied = std::io::copy(
                &mut self.inner.take(self.remaining as u64),
                &mut std::io::sink(),
            )?;
            self.remaining -= copied as usize;
        }
        Ok(())
    }

    /// Number of bytes remaining in the bound.
    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

impl Read for BoundedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = buf.len().min(self.remaining);
        let n = self.inner.read(&mut buf[..limit])?;
        self.remaining -= n;
        Ok(n)
    }
}

/// Trait for types that can serialize/deserialize with inherent bounds checking.
///
/// Unlike `serde::Deserialize`, this trait passes a `BoundedReader` so the
/// deserialization is implicitly bounded by the caller-provided limit.
pub trait SafeSerializable: Sized {
    fn serialize(&self, writer: &mut impl std::io::Write) -> StorageResult<()>;
    fn deserialize(reader: &mut BoundedReader<'_>) -> StorageResult<Self>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_reader_within_limit() {
        let data = b"hello world";
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 20);
        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        assert_eq!(reader.remaining(), 15);
    }

    #[test]
    fn test_bounded_reader_exceeds_limit() {
        let data = b"hello world";
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 3);
        let mut buf = [0u8; 5];
        let result = reader.read_exact(&mut buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_bounded_reader_skip_all() {
        let data = [0u8; 32];
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 20);
        reader.skip_all().unwrap();
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn test_bounded_reader_read_trait() {
        let data = b"hello world";
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 5);
        let mut buf = [0u8; 10];
        let n = reader.read(&mut buf).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], b"hello");
        assert_eq!(reader.remaining(), 0);
        let n2 = reader.read(&mut buf).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn test_bounded_reader_exact_limit() {
        let data = b"hello";
        let mut cursor = std::io::Cursor::new(&data[..]);
        let mut reader = BoundedReader::new(&mut cursor, 5);
        let mut buf = [0u8; 5];
        reader.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        assert_eq!(reader.remaining(), 0);
    }
}

//! WAL compression strategy

use crate::core::wal::types::{WalCompression, WalConfig, WalError, WalResult};

/// Compression strategy trait
pub(crate) trait Compressor: Send + Sync {
    fn compress(&self, data: &[u8]) -> WalResult<(Vec<u8>, WalCompression)>;
}

/// No-op compressor (no compression)
pub(crate) struct NoopCompressor;

impl Compressor for NoopCompressor {
    fn compress(&self, data: &[u8]) -> WalResult<(Vec<u8>, WalCompression)> {
        Ok((data.to_vec(), WalCompression::None))
    }
}

/// Zstd compressor
pub(crate) struct ZstdCompressor {
    level: i32,
    min_size: usize,
}

impl ZstdCompressor {
    pub fn new(level: i32, min_size: usize) -> Self {
        Self { level, min_size }
    }
}

impl Compressor for ZstdCompressor {
    fn compress(&self, data: &[u8]) -> WalResult<(Vec<u8>, WalCompression)> {
        if data.len() < self.min_size {
            return Ok((data.to_vec(), WalCompression::None));
        }

        let compressed = zstd::encode_all(data, self.level)
            .map_err(|e| WalError::SerializationError(e.to_string()))?;

        if compressed.len() < data.len() {
            Ok((compressed, WalCompression::Zstd))
        } else {
            Ok((data.to_vec(), WalCompression::None))
        }
    }
}

/// Create a compressor based on configuration
pub(crate) fn create_compressor(config: &WalConfig) -> Box<dyn Compressor> {
    match config.compression {
        WalCompression::Zstd => Box::new(ZstdCompressor::new(
            config.compression_level.level as i32,
            64,
        )),
        WalCompression::None => Box::new(NoopCompressor),
    }
}

/// Decompress a payload (public helper)
pub fn decompress_payload(payload: &[u8], compression: WalCompression) -> WalResult<Vec<u8>> {
    match compression {
        WalCompression::Zstd => {
            zstd::decode_all(payload).map_err(|e| WalError::DeserializationError(e.to_string()))
        }
        WalCompression::None => Ok(payload.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_compressor() {
        let compressor = NoopCompressor;
        let data = b"hello world";
        let (compressed, compression) = compressor.compress(data).unwrap();
        assert_eq!(compression, WalCompression::None);
        assert_eq!(compressed.as_slice(), data);
    }

    #[test]
    fn test_zstd_compressor_small_data() {
        let compressor = ZstdCompressor::new(3, 64);
        let data = b"small";
        let (compressed, compression) = compressor.compress(data).unwrap();
        // Data too small, should not compress
        assert_eq!(compression, WalCompression::None);
        assert_eq!(compressed, data);
    }

    #[test]
    fn test_decompress_payload_public() {
        let data = b"test data";
        let result = decompress_payload(data, WalCompression::None).unwrap();
        assert_eq!(result, data);
    }
}

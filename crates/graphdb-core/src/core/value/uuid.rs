//! UUID Type Module - Graph Database UUID Support
//!
//! This module provides UUID (Universally Unique Identifier) type support
//! following RFC 4122 standard.
//!
//! ## Features
//! - Standard UUID format (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
//! - Binary storage (16 bytes)
//! - Fast comparison and hashing
//! - PostgreSQL compatible

use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;
use std::str::FromStr;

/// UUID Value Type (16 bytes)
///
/// Stores UUID in binary format for efficient storage and comparison.
/// Supports all UUID versions (1, 3, 4, 5, 6, 7, 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UuidValue(pub [u8; 16]);

impl UuidValue {
    /// Create UUID from raw bytes
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Create UUID from slice (returns error if slice length != 16)
    pub fn from_slice(slice: &[u8]) -> Result<Self, UuidError> {
        if slice.len() != 16 {
            return Err(UuidError::InvalidLength(slice.len()));
        }
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(slice);
        Ok(Self(bytes))
    }

    /// Parse UUID from string (standard format: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
    pub fn parse_str(s: &str) -> Result<Self, UuidError> {
        // Remove hyphens and validate length
        let hex_str: String = s.chars().filter(|&c| c != '-').collect();

        if hex_str.len() != 32 {
            return Err(UuidError::InvalidFormat(s.to_string()));
        }

        // Parse hex string to bytes
        let mut bytes = [0u8; 16];
        for (i, chunk) in hex_str.as_bytes().chunks(2).enumerate() {
            let hex_chunk =
                std::str::from_utf8(chunk).map_err(|_| UuidError::InvalidFormat(s.to_string()))?;
            bytes[i] = u8::from_str_radix(hex_chunk, 16)
                .map_err(|_| UuidError::InvalidFormat(s.to_string()))?;
        }

        Ok(Self(bytes))
    }

    /// Generate a new UUID v4 (random)]
    pub fn new_v4() -> Self {
        use rand::Rng;
        let mut bytes = [0u8; 16];
        rand::thread_rng().fill(&mut bytes);

        // Set version (4) and variant (RFC 4122)
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // Version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // Variant 10

        Self(bytes)
    }

    /// Get raw bytes
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Convert to bytes array
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Get UUID version (bits 12-15 of time_hi_and_version field)
    pub fn version(&self) -> u8 {
        (self.0[6] >> 4) & 0x0f
    }

    /// Get UUID variant
    pub fn variant(&self) -> UuidVariant {
        match self.0[8] >> 6 {
            0b00 => UuidVariant::NCS,
            0b10 => UuidVariant::RFC4122,
            0b11 => UuidVariant::Microsoft,
            _ => UuidVariant::Future,
        }
    }

    /// Format as hyphenated string (xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
    pub fn to_hyphenated_string(&self) -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3],
            self.0[4], self.0[5],
            self.0[6], self.0[7],
            self.0[8], self.0[9],
            self.0[10], self.0[11], self.0[12], self.0[13], self.0[14], self.0[15]
        )
    }

    /// Format as simple string (no hyphens)
    pub fn to_simple_string(&self) -> String {
        format!(
            "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3],
            self.0[4], self.0[5], self.0[6], self.0[7],
            self.0[8], self.0[9], self.0[10], self.0[11],
            self.0[12], self.0[13], self.0[14], self.0[15]
        )
    }

    /// Format as URN string (urn:uuid:xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx)
    pub fn to_urn_string(&self) -> String {
        format!("urn:uuid:{}", self.to_hyphenated_string())
    }

    /// Estimate memory usage (always 16 bytes)
    pub const fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    /// Nil UUID (all zeros)
    pub const fn nil() -> Self {
        Self([0u8; 16])
    }

    /// Max UUID (all ones)
    pub const fn max() -> Self {
        Self([0xffu8; 16])
    }
}

impl Default for UuidValue {
    fn default() -> Self {
        Self::nil()
    }
}

impl fmt::Display for UuidValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hyphenated_string())
    }
}

impl FromStr for UuidValue {
    type Err = UuidError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse_str(s)
    }
}

impl AsRef<[u8]> for UuidValue {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<[u8; 16]> for UuidValue {
    fn from(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl From<UuidValue> for [u8; 16] {
    fn from(uuid: UuidValue) -> Self {
        uuid.0
    }
}

/// UUID Variant
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UuidVariant {
    /// NCS compatibility variant (0b0xx)
    NCS,
    /// RFC 4122 variant (0b10x)
    RFC4122,
    /// Microsoft variant (0b110)
    Microsoft,
    /// Future reserved variant (0b111)
    Future,
}

/// UUID Error Type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UuidError {
    InvalidLength(usize),
    InvalidFormat(String),
}

impl fmt::Display for UuidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UuidError::InvalidLength(len) => {
                write!(f, "Invalid UUID length: expected 16, got {}", len)
            }
            UuidError::InvalidFormat(s) => write!(f, "Invalid UUID format: {}", s),
        }
    }
}

impl std::error::Error for UuidError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uuid_parse() {
        let uuid_str = "550e8400-e29b-41d4-a716-446655440000";
        let uuid = UuidValue::parse_str(uuid_str).unwrap();
        assert_eq!(uuid.to_hyphenated_string(), uuid_str);
    }

    #[test]
    fn test_uuid_parse_simple() {
        let uuid_str = "550e8400e29b41d4a716446655440000";
        let uuid = UuidValue::parse_str(uuid_str).unwrap();
        assert_eq!(uuid.to_simple_string(), uuid_str);
    }

    #[test]
    fn test_uuid_version() {
        // Version 4 UUID: 550e8400-e29b-41d4-a716-446655440000
        let uuid = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid.version(), 4);
    }

    #[test]
    fn test_uuid_variant() {
        let uuid = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid.variant(), UuidVariant::RFC4122);
    }

    #[test]
    fn test_uuid_nil() {
        let nil = UuidValue::nil();
        assert_eq!(nil.to_simple_string(), "00000000000000000000000000000000");
    }

    #[test]
    fn test_uuid_max() {
        let max = UuidValue::max();
        assert_eq!(max.to_simple_string(), "ffffffffffffffffffffffffffffffff");
    }

    #[test]
    fn test_uuid_from_bytes() {
        let bytes = [
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ];
        let uuid = UuidValue::from_bytes(bytes);
        assert_eq!(uuid.as_bytes(), &bytes);
    }

    #[test]
    fn test_uuid_invalid_format() {
        let result = UuidValue::parse_str("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_uuid_invalid_length() {
        let result = UuidValue::from_slice(&[1, 2, 3]);
        assert!(result.is_err());
    }

    #[test]
    fn test_uuid_display() {
        let uuid = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(format!("{}", uuid), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn test_uuid_urn() {
        let uuid = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(
            uuid.to_urn_string(),
            "urn:uuid:550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn test_uuid_equality() {
        let uuid1 = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let uuid2 = UuidValue::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let uuid3 = UuidValue::parse_str("660e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid1, uuid2);
        assert_ne!(uuid1, uuid3);
    }

    #[test]
    fn test_uuid_from_str_trait() {
        let uuid: UuidValue = "550e8400-e29b-41d4-a716-446655440000".parse().unwrap();
        assert_eq!(
            uuid.to_hyphenated_string(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }
}

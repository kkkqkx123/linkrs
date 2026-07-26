//! Column Statistics
//!
//! Persistent column statistics for query optimization.
//! Provides min/max values, null counts, and encoding metadata
//! that can be used for predicate pushdown and range pruning.

use std::collections::HashSet;
use std::io::{Read, Write};

use crate::core::{StorageResult, Value};
use crate::storage::encoding::EncodingType;

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStats {
    pub min_value: Option<Value>,
    pub max_value: Option<Value>,
    pub null_count: u64,
    pub distinct_count: Option<u64>,
    pub encoding_type: EncodingType,
    pub compressed_size: u64,
    pub raw_size: u64,
}

impl ColumnStats {
    pub fn new(encoding_type: EncodingType, compressed_size: u64, raw_size: u64) -> Self {
        Self {
            min_value: None,
            max_value: None,
            null_count: 0,
            distinct_count: None,
            encoding_type,
            compressed_size,
            raw_size,
        }
    }

    pub fn compression_ratio(&self) -> f64 {
        if self.raw_size == 0 {
            1.0
        } else {
            self.compressed_size as f64 / self.raw_size as f64
        }
    }

    pub fn space_savings(&self) -> f64 {
        if self.raw_size == 0 {
            0.0
        } else {
            1.0 - self.compressed_size as f64 / self.raw_size as f64
        }
    }

    pub fn serialize_meta(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;

        writer.write_all(&[self.min_value.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.min_value {
            written += serialize_value(writer, v)?;
        }

        writer.write_all(&[self.max_value.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.max_value {
            written += serialize_value(writer, v)?;
        }

        writer.write_all(&self.null_count.to_le_bytes())?;
        written += 8;

        writer.write_all(&[self.distinct_count.is_some() as u8])?;
        written += 1;
        if let Some(d) = self.distinct_count {
            writer.write_all(&d.to_le_bytes())?;
            written += 8;
        }

        writer.write_all(&[self.encoding_type.to_u8()])?;
        written += 1;

        writer.write_all(&self.compressed_size.to_le_bytes())?;
        written += 8;

        writer.write_all(&self.raw_size.to_le_bytes())?;
        written += 8;

        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut buf = [0u8; 1];

        reader.read_exact(&mut buf)?;
        let has_min = buf[0] != 0;
        let min_value = if has_min {
            Some(deserialize_value(reader)?)
        } else {
            None
        };

        reader.read_exact(&mut buf)?;
        let has_max = buf[0] != 0;
        let max_value = if has_max {
            Some(deserialize_value(reader)?)
        } else {
            None
        };

        let mut nb = [0u8; 8];
        reader.read_exact(&mut nb)?;
        let null_count = u64::from_le_bytes(nb);

        reader.read_exact(&mut buf)?;
        let has_distinct = buf[0] != 0;
        let distinct_count = if has_distinct {
            let mut db = [0u8; 8];
            reader.read_exact(&mut db)?;
            Some(u64::from_le_bytes(db))
        } else {
            None
        };

        reader.read_exact(&mut buf)?;
        let encoding_type = EncodingType::from_u8(buf[0]);

        reader.read_exact(&mut nb)?;
        let compressed_size = u64::from_le_bytes(nb);

        reader.read_exact(&mut nb)?;
        let raw_size = u64::from_le_bytes(nb);

        Ok(Self {
            min_value,
            max_value,
            null_count,
            distinct_count,
            encoding_type,
            compressed_size,
            raw_size,
        })
    }
}

fn serialize_value(writer: &mut impl Write, value: &Value) -> StorageResult<usize> {
    match value {
        Value::SmallInt(v) => {
            writer.write_all(&[1u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(9)
        }
        Value::Int(v) => {
            writer.write_all(&[2u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(5)
        }
        Value::BigInt(v) => {
            writer.write_all(&[3u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(9)
        }
        Value::Float(v) => {
            writer.write_all(&[4u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(5)
        }
        Value::Double(v) => {
            writer.write_all(&[5u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(9)
        }
        Value::Bool(v) => {
            writer.write_all(&[6u8])?;
            writer.write_all(&[*v as u8])?;
            Ok(2)
        }
        Value::String(s) => {
            writer.write_all(&[7u8])?;
            let bytes = s.as_bytes();
            writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bytes)?;
            Ok(5 + bytes.len())
        }
        _ => Err(crate::core::StorageError::not_supported(format!(
            "Stats serialization for value type {:?}",
            value.data_type()
        ))),
    }
}

fn deserialize_value(reader: &mut impl Read) -> StorageResult<Value> {
    let mut tag = [0u8; 1];
    reader.read_exact(&mut tag)?;

    match tag[0] {
        1 => {
            let mut b = [0u8; 2];
            reader.read_exact(&mut b)?;
            Ok(Value::SmallInt(i16::from_le_bytes(b)))
        }
        2 => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b)?;
            Ok(Value::Int(i32::from_le_bytes(b)))
        }
        3 => {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b)?;
            Ok(Value::BigInt(i64::from_le_bytes(b)))
        }
        4 => {
            let mut b = [0u8; 4];
            reader.read_exact(&mut b)?;
            Ok(Value::Float(f32::from_le_bytes(b)))
        }
        5 => {
            let mut b = [0u8; 8];
            reader.read_exact(&mut b)?;
            Ok(Value::Double(f64::from_le_bytes(b)))
        }
        6 => {
            let mut b = [0u8; 1];
            reader.read_exact(&mut b)?;
            Ok(Value::Bool(b[0] != 0))
        }
        7 => {
            let mut lb = [0u8; 4];
            reader.read_exact(&mut lb)?;
            let len = u32::from_le_bytes(lb) as usize;
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let s = String::from_utf8(bytes)
                .map_err(|e| crate::core::StorageError::deserialize_error(e.to_string()))?;
            Ok(Value::string(s))
        }
        _ => Err(crate::core::StorageError::deserialize_error(format!(
            "Unknown value tag {} in stats",
            tag[0]
        ))),
    }
}

pub fn compute_stats(
    values: &[Option<Value>],
    encoding_type: EncodingType,
    compressed_size: u64,
    raw_size: u64,
) -> ColumnStats {
    let mut stats = ColumnStats::new(encoding_type, compressed_size, raw_size);

    let mut distinct = HashSet::new();

    for v in values {
        match v {
            Some(val) => {
                distinct.insert(val.clone());

                if let Some(ref min) = stats.min_value {
                    if val < min {
                        stats.min_value = Some(val.clone());
                    }
                } else {
                    stats.min_value = Some(val.clone());
                }

                if let Some(ref max) = stats.max_value {
                    if val > max {
                        stats.max_value = Some(val.clone());
                    }
                } else {
                    stats.max_value = Some(val.clone());
                }
            }
            None => {
                stats.null_count += 1;
            }
        }
    }

    stats.distinct_count = Some(distinct.len() as u64);
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;

    #[test]
    fn test_stats_serialize_roundtrip() {
        let mut stats = ColumnStats::new(EncodingType::Alp, 1024, 4096);
        stats.min_value = Some(Value::Double(1.5));
        stats.max_value = Some(Value::Double(99.5));
        stats.null_count = 3;
        stats.distinct_count = Some(50);

        let mut buf = Vec::new();
        stats.serialize_meta(&mut buf).unwrap();

        let mut cursor = &buf[..];
        let restored = ColumnStats::deserialize_meta(&mut cursor).unwrap();

        assert_eq!(restored.min_value, stats.min_value);
        assert_eq!(restored.max_value, stats.max_value);
        assert_eq!(restored.null_count, stats.null_count);
        assert_eq!(restored.distinct_count, stats.distinct_count);
        assert_eq!(restored.encoding_type, stats.encoding_type);
        assert_eq!(restored.compressed_size, stats.compressed_size);
        assert_eq!(restored.raw_size, stats.raw_size);
    }

    #[test]
    fn test_stats_compute() {
        let values = vec![
            Some(Value::Int(10)),
            Some(Value::Int(20)),
            None,
            Some(Value::Int(5)),
            Some(Value::Int(20)),
        ];

        let stats = compute_stats(&values, EncodingType::BitPacking, 100, 200);

        assert_eq!(stats.min_value, Some(Value::Int(5)));
        assert_eq!(stats.max_value, Some(Value::Int(20)));
        assert_eq!(stats.null_count, 1);
        assert_eq!(stats.distinct_count, Some(3));
        assert_eq!(stats.encoding_type, EncodingType::BitPacking);
    }

    #[test]
    fn test_stats_with_strings() {
        let mut stats = ColumnStats::new(EncodingType::Dictionary, 512, 2048);
        stats.min_value = Some(Value::string("apple"));
        stats.max_value = Some(Value::string("zebra"));

        let mut buf = Vec::new();
        stats.serialize_meta(&mut buf).unwrap();

        let mut cursor = &buf[..];
        let restored = ColumnStats::deserialize_meta(&mut cursor).unwrap();

        assert_eq!(restored.min_value, stats.min_value);
        assert_eq!(restored.max_value, stats.max_value);
    }
}

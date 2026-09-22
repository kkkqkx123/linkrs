//! Column Statistics
//!
//! Persistent column statistics for query optimization.
//! Provides min/max values, null counts, and encoding metadata
//! that can be used for predicate pushdown and range pruning.
//!
//! Truncated files are rejected; there is no silent defaulting.

use std::io::{Read, Write};

use crate::encoding::EncodingType;
use crate::stats::HyperLogLog;
use graphdb_core::{StorageResult, Value};

/// Persistent format version of the column statistics payload.
///
/// Written as the first byte by `serialize_meta` and checked first by
/// `deserialize_meta`. Unknown versions are rejected without migration:
/// old payloads never silently decode under a newer layout.
pub const COLUMN_STATS_FORMAT_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStats {
    pub min_value: Option<Value>,
    pub max_value: Option<Value>,
    pub null_count: u64,
    pub distinct_count: Option<u64>,
    pub encoding_type: EncodingType,
    pub compressed_size: u64,
    pub raw_size: u64,
    /// HLL registers backing the distinct estimate (64 bytes when present).
    pub hll: Option<HyperLogLog>,
    /// True when the column provably contains no nulls.
    pub guaranteed_no_nulls: bool,
    /// True when every observed row is null.
    pub all_null: bool,
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
            hll: None,
            guaranteed_no_nulls: false,
            all_null: false,
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

        writer.write_all(&[COLUMN_STATS_FORMAT_VERSION])?;
        written += 1;

        writer.write_all(&[self.min_value.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.min_value {
            written += serialize_stat_value(writer, v)?;
        }

        writer.write_all(&[self.max_value.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.max_value {
            written += serialize_stat_value(writer, v)?;
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

        writer.write_all(&[self.hll.is_some() as u8])?;
        written += 1;
        if let Some(ref hll) = self.hll {
            written += hll.serialize(writer)?;
        }
        writer.write_all(&[self.guaranteed_no_nulls as u8])?;
        writer.write_all(&[self.all_null as u8])?;
        written += 2;

        Ok(written)
    }

    pub fn deserialize_meta(reader: &mut impl Read) -> StorageResult<Self> {
        let mut buf = [0u8; 1];

        // Version gate first: payloads written without a version byte or
        // under another version are rejected instead of misdecoded.
        reader.read_exact(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                graphdb_core::StorageError::deserialize_error(
                    "ColumnStats truncated: missing format version".to_string(),
                )
            } else {
                graphdb_core::StorageError::io_error(e.to_string())
            }
        })?;
        if buf[0] != COLUMN_STATS_FORMAT_VERSION {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "unsupported ColumnStats format version {}, expected {}; old formats are rejected without migration",
                buf[0], COLUMN_STATS_FORMAT_VERSION
            )));
        }

        reader.read_exact(&mut buf)?;
        let has_min = buf[0] != 0;
        let min_value = if has_min {
            Some(deserialize_stat_value(reader)?)
        } else {
            None
        };

        reader.read_exact(&mut buf)?;
        let has_max = buf[0] != 0;
        let max_value = if has_max {
            Some(deserialize_stat_value(reader)?)
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

        // HLL tail and null flags are mandatory; truncated files are
        // rejected, never silently defaulted.
        let mut tail = [0u8; 1];
        reader.read_exact(&mut tail).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                graphdb_core::StorageError::deserialize_error(
                    "ColumnStats truncated: missing HLL tail".to_string(),
                )
            } else {
                graphdb_core::StorageError::io_error(e.to_string())
            }
        })?;
        let hll = if tail[0] != 0 {
            Some(HyperLogLog::deserialize(reader)?)
        } else {
            None
        };
        let mut flags = [0u8; 2];
        reader.read_exact(&mut flags).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                graphdb_core::StorageError::deserialize_error(
                    "ColumnStats truncated: missing null flags".to_string(),
                )
            } else {
                graphdb_core::StorageError::io_error(e.to_string())
            }
        })?;
        let (guaranteed_no_nulls, all_null) = (flags[0] != 0, flags[1] != 0);

        Ok(Self {
            min_value,
            max_value,
            null_count,
            distinct_count,
            encoding_type,
            compressed_size,
            raw_size,
            hll,
            guaranteed_no_nulls,
            all_null,
        })
    }
}

pub(crate) fn serialize_stat_value(writer: &mut impl Write, value: &Value) -> StorageResult<usize> {
    match value {
        Value::SmallInt(v) => {
            writer.write_all(&[1u8])?;
            writer.write_all(&v.to_le_bytes())?;
            Ok(3)
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
        Value::Date(d) => {
            writer.write_all(&[8u8])?;
            writer.write_all(&d.year.to_le_bytes())?;
            writer.write_all(&d.month.to_le_bytes())?;
            writer.write_all(&d.day.to_le_bytes())?;
            Ok(13)
        }
        Value::Time(t) => {
            writer.write_all(&[9u8])?;
            writer.write_all(&t.hour.to_le_bytes())?;
            writer.write_all(&t.minute.to_le_bytes())?;
            writer.write_all(&t.sec.to_le_bytes())?;
            writer.write_all(&t.microsec.to_le_bytes())?;
            Ok(17)
        }
        Value::DateTime(dt) => {
            writer.write_all(&[10u8])?;
            writer.write_all(&dt.year.to_le_bytes())?;
            writer.write_all(&dt.month.to_le_bytes())?;
            writer.write_all(&dt.day.to_le_bytes())?;
            writer.write_all(&dt.hour.to_le_bytes())?;
            writer.write_all(&dt.minute.to_le_bytes())?;
            writer.write_all(&dt.sec.to_le_bytes())?;
            writer.write_all(&dt.microsec.to_le_bytes())?;
            Ok(29)
        }
        Value::Uuid(u) => {
            writer.write_all(&[11u8])?;
            writer.write_all(u.as_bytes())?;
            Ok(17)
        }
        Value::FixedString(s) => {
            writer.write_all(&[12u8])?;
            let bytes = s.as_bytes();
            writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bytes)?;
            Ok(5 + bytes.len())
        }
        Value::Json(j) => {
            writer.write_all(&[13u8])?;
            let bytes = j.as_str().as_bytes();
            writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bytes)?;
            Ok(5 + bytes.len())
        }
        Value::JsonB(j) => {
            writer.write_all(&[13u8])?;
            let text = j.to_json_string();
            let bytes = text.as_bytes();
            writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
            writer.write_all(bytes)?;
            Ok(5 + bytes.len())
        }
        Value::Blob(b) => {
            writer.write_all(&[14u8])?;
            writer.write_all(&(b.len() as u32).to_le_bytes())?;
            writer.write_all(b)?;
            Ok(5 + b.len())
        }
        // Struct/Array have no meaningful ordering; skip min/max serialization.
        // Callers should use stat_orderable() to exclude these from min/max
        // tracking, but this is a safety net.
        Value::Struct(_) | Value::Array(_) => {
            writer.write_all(&[15u8])?;
            Ok(1)
        }
        _ => Err(graphdb_core::StorageError::not_supported(format!(
            "Stats serialization for value type {:?}",
            value.data_type()
        ))),
    }
}

pub(crate) fn deserialize_stat_value(reader: &mut impl Read) -> StorageResult<Value> {
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
                .map_err(|e| graphdb_core::StorageError::deserialize_error(e.to_string()))?;
            Ok(Value::string(s))
        }
        8 => {
            let mut i32b = [0u8; 4];
            let mut u32b = [0u8; 4];
            reader.read_exact(&mut i32b)?;
            let year = i32::from_le_bytes(i32b);
            reader.read_exact(&mut u32b)?;
            let month = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let day = u32::from_le_bytes(u32b);
            Ok(Value::Date(graphdb_core::value::DateValue {
                year,
                month,
                day,
            }))
        }
        9 => {
            let mut u32b = [0u8; 4];
            reader.read_exact(&mut u32b)?;
            let hour = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let minute = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let sec = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let microsec = u32::from_le_bytes(u32b);
            Ok(Value::Time(graphdb_core::value::TimeValue {
                hour,
                minute,
                sec,
                microsec,
            }))
        }
        10 => {
            let mut i32b = [0u8; 4];
            let mut u32b = [0u8; 4];
            reader.read_exact(&mut i32b)?;
            let year = i32::from_le_bytes(i32b);
            reader.read_exact(&mut u32b)?;
            let month = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let day = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let hour = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let minute = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let sec = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let microsec = u32::from_le_bytes(u32b);
            Ok(Value::DateTime(graphdb_core::value::DateTimeValue {
                year,
                month,
                day,
                hour,
                minute,
                sec,
                microsec,
            }))
        }
        11 => {
            let mut bytes = [0u8; 16];
            reader.read_exact(&mut bytes)?;
            Ok(Value::Uuid(graphdb_core::value::UuidValue::from_bytes(
                bytes,
            )))
        }
        12 => {
            let mut lb = [0u8; 4];
            reader.read_exact(&mut lb)?;
            let len = u32::from_le_bytes(lb) as usize;
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let s = String::from_utf8(bytes)
                .map_err(|e| graphdb_core::StorageError::deserialize_error(e.to_string()))?;
            Ok(Value::FixedString(s))
        }
        13 => {
            let mut lb = [0u8; 4];
            reader.read_exact(&mut lb)?;
            let len = u32::from_le_bytes(lb) as usize;
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            let s = String::from_utf8(bytes)
                .map_err(|e| graphdb_core::StorageError::deserialize_error(e.to_string()))?;
            match Value::json(&s) {
                Ok(v) => Ok(v),
                Err(e) => Err(graphdb_core::StorageError::deserialize_error(e.to_string())),
            }
        }
        14 => {
            let mut lb = [0u8; 4];
            reader.read_exact(&mut lb)?;
            let len = u32::from_le_bytes(lb) as usize;
            let mut bytes = vec![0u8; len];
            reader.read_exact(&mut bytes)?;
            Ok(Value::Blob(bytes))
        }
        // Struct/Array stats placeholder (min/max skipped during serialization).
        // Return empty string as a safe placeholder; callers should not rely on
        // this value for Struct/Array columns.
        15 => Ok(Value::string("")),
        _ => Err(graphdb_core::StorageError::deserialize_error(format!(
            "Unknown value tag {} in stats",
            tag[0]
        ))),
    }
}

/// Whether a value kind has stats min/max ordering and serialization.
/// Composite, graph, vector, decimal and interval kinds keep counts and HLL
/// only; Json/JsonB record string-order bounds without pruning guarantees.
pub fn stat_orderable(value: &Value) -> bool {
    matches!(
        value,
        Value::SmallInt(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Double(_)
            | Value::Bool(_)
            | Value::String(_)
            | Value::Date(_)
            | Value::Time(_)
            | Value::DateTime(_)
            | Value::Uuid(_)
            | Value::FixedString(_)
            | Value::Json(_)
            | Value::JsonB(_)
            | Value::Blob(_)
    )
}

/// Streaming aggregation over an iterator: single pass, no value vector,
/// HLL-backed distinct estimate instead of a hash set.
pub fn compute_stats_streaming(
    values: impl Iterator<Item = Option<Value>>,
    encoding_type: EncodingType,
    compressed_size: u64,
    raw_size: u64,
) -> ColumnStats {
    let mut stats = ColumnStats::new(encoding_type, compressed_size, raw_size);
    let mut hll = HyperLogLog::new();
    let mut total = 0u64;

    for v in values {
        match v {
            Some(val) => {
                total += 1;
                hll.add_value(&val);
                if stat_orderable(&val) {
                    if let Some(ref min) = stats.min_value {
                        if val < *min {
                            stats.min_value = Some(val.clone());
                        }
                    } else {
                        stats.min_value = Some(val.clone());
                    }
                    if let Some(ref max) = stats.max_value {
                        if val > *max {
                            stats.max_value = Some(val.clone());
                        }
                    } else {
                        stats.max_value = Some(val.clone());
                    }
                }
            }
            None => {
                stats.null_count += 1;
            }
        }
    }

    stats.distinct_count = Some(hll.estimate());
    stats.hll = Some(hll);
    stats.all_null = total == 0 && stats.null_count > 0;
    stats.guaranteed_no_nulls = stats.null_count == 0;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::Value;

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

        let stats = compute_stats_streaming(values.into_iter(), EncodingType::BitPacking, 100, 200);

        assert_eq!(stats.min_value, Some(Value::Int(5)));
        assert_eq!(stats.max_value, Some(Value::Int(20)));
        assert_eq!(stats.null_count, 1);
        assert!(stats.distinct_count.unwrap() >= 2);
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

    #[test]
    fn test_new_scalar_tags_roundtrip() {
        let cases: Vec<Value> = vec![
            Value::Date(graphdb_core::value::DateValue {
                year: 2024,
                month: 2,
                day: 29,
            }),
            Value::Time(graphdb_core::value::TimeValue {
                hour: 1,
                minute: 2,
                sec: 3,
                microsec: 4,
            }),
            Value::DateTime(graphdb_core::value::DateTimeValue {
                year: 2024,
                month: 1,
                day: 2,
                hour: 3,
                minute: 4,
                sec: 5,
                microsec: 6,
            }),
            Value::Uuid(graphdb_core::value::UuidValue::from_bytes([7u8; 16])),
            Value::FixedString("abc".to_string()),
            Value::Blob(vec![1, 2, 3]),
        ];
        for v in cases {
            let mut buf = Vec::new();
            serialize_stat_value(&mut buf, &v).unwrap();
            let back = deserialize_stat_value(&mut &buf[..]).unwrap();
            assert_eq!(back, v);
        }
        let j = Value::json(r#"{"a":1}"#).unwrap();
        let mut buf = Vec::new();
        serialize_stat_value(&mut buf, &j).unwrap();
        assert!(deserialize_stat_value(&mut &buf[..]).is_ok());
    }

    #[test]
    fn test_streaming_matches_min_max_null() {
        let values = vec![
            Some(Value::Int(10)),
            Some(Value::Int(20)),
            None,
            Some(Value::Int(5)),
            Some(Value::Int(20)),
        ];
        let stats = compute_stats_streaming(values.into_iter(), EncodingType::BitPacking, 100, 200);
        assert_eq!(stats.min_value, Some(Value::Int(5)));
        assert_eq!(stats.max_value, Some(Value::Int(20)));
        assert_eq!(stats.null_count, 1);
        assert!(!stats.guaranteed_no_nulls);
    }

    #[test]
    fn test_hll_merge_accuracy() {
        let mut a = HyperLogLog::new();
        let mut b = HyperLogLog::new();
        for i in 0..5000i32 {
            if i % 2 == 0 {
                a.add_value(&Value::Int(i));
            } else {
                b.add_value(&Value::Int(i));
            }
        }
        a.merge(&b);
        let est = a.estimate() as f64;
        let err = (est - 5000.0).abs() / 5000.0;
        assert!(err < 0.15, "estimate={}", est);
    }

    #[test]
    fn test_truncated_stats_reject() {
        let mut stats = ColumnStats::new(EncodingType::Alp, 1024, 4096);
        stats.min_value = Some(Value::Double(1.5));
        let mut buf = Vec::new();
        stats.serialize_meta(&mut buf).unwrap();
        let truncated = &buf[..buf.len() - 1];
        let err = ColumnStats::deserialize_meta(&mut &truncated[..]).unwrap_err();
        assert!(err.to_string().contains("truncated"));
    }

    #[test]
    fn test_unknown_format_version_rejected() {
        let mut stats = ColumnStats::new(EncodingType::Alp, 1024, 4096);
        stats.min_value = Some(Value::Double(1.5));
        let mut buf = Vec::new();
        stats.serialize_meta(&mut buf).unwrap();
        assert_eq!(buf[0], COLUMN_STATS_FORMAT_VERSION);
        // Payloads from another version never decode: the gate rejects
        // before any field is read.
        buf[0] = COLUMN_STATS_FORMAT_VERSION.wrapping_add(1);
        let err = ColumnStats::deserialize_meta(&mut &buf[..]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("unsupported ColumnStats format version"));
        assert!(msg.contains("rejected without migration"));
        // A version-less prefix (legacy layout) fails at the same gate:
        // with no bounds recorded the first byte is zero, never the version.
        let bare = ColumnStats::new(EncodingType::Alp, 1024, 4096);
        let mut bare_buf = Vec::new();
        bare.serialize_meta(&mut bare_buf).unwrap();
        let legacy = &bare_buf[1..];
        let err = ColumnStats::deserialize_meta(&mut &legacy[..]).unwrap_err();
        assert!(err
            .to_string()
            .contains("unsupported ColumnStats format version"));
    }
}

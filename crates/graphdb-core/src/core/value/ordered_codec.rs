//! Ordered key codec — order-preserving binary encoding for `Value`.
//!
//! The byte-level comparison of encoded values matches the semantic ordering
//! of `Value` (as defined by `Value::cmp`).  This enables range and prefix
//! scans on index keys without post-filtering.
//!
//! # Format (version 1)
//!
//! Each field starts with a type tag byte, followed by the encoded body.
//! Fixed-length types encode to a fixed byte count; variable-length types
//! (String, Blob) use a 0x00 terminator byte.  The 0x00 byte does not appear
//! inside the encoded data — encoding a value containing 0x00 returns an
//! [`StorageError`].
//!
//! The terminator enables tight prefix upper bounds: for a string or blob
//! prefix value P, the range scan bound is `[encode(P), tag +
//! prefix_upper_bound(P_data_bytes))`, covering all values that start with P
//! and excluding those that do not.
//!
//! Composite keys concatenate multiple field encodings; the entity tie-breaker
//! is appended as the final field.

use crate::core::types::storage_ids::VertexId;
use crate::core::value::date_time::{DateTimeValue, DateValue, TimeValue};
use crate::core::value::UuidValue;
use crate::core::wal::EntityRef;
use crate::core::{NullType, StorageError, Value};

/// Current wire format version.
pub const ORDERED_CODEC_VERSION: u8 = 0x01;

// ── Type tags (1 byte) ──────────────────────────────────────────────────────

const TAG_NULL: u8 = 0x00;
const TAG_BOOL: u8 = 0x01;
const TAG_SMALL_INT: u8 = 0x02;
const TAG_INT: u8 = 0x03;
const TAG_BIG_INT: u8 = 0x04;
const TAG_FLOAT: u8 = 0x05;
const TAG_DOUBLE: u8 = 0x06;
const TAG_STRING: u8 = 0x07;
const TAG_BLOB: u8 = 0x08;
const TAG_DATE: u8 = 0x09;
const TAG_TIME: u8 = 0x0A;
const TAG_DATE_TIME: u8 = 0x0B;
const TAG_UUID: u8 = 0x0C;
const TAG_VERTEX_ID: u8 = 0x0D;
const TAG_EDGE_REF: u8 = 0x0E;
const TAG_NULL_LAST: u8 = 0xFF;

/// Order-preserving codec for index keys.
///
/// Stateless: can be shared and reused freely.
#[derive(Debug, Clone, Copy)]
pub struct OrderedCodec {
    version: u8,
}

impl OrderedCodec {
    pub const fn new() -> Self {
        Self {
            version: ORDERED_CODEC_VERSION,
        }
    }

    pub const fn version(&self) -> u8 {
        self.version
    }

    /// Encode a single `Value` into order-preserving bytes.
    pub fn encode(&self, value: &Value) -> Result<Vec<u8>, StorageError> {
        let mut buf = Vec::new();
        self.encode_value(value, &mut buf, false)?;
        Ok(buf)
    }

    /// Encode value with null-last placement (sorts after all non-null values).
    pub fn encode_null_last(&self, value: &Value) -> Result<Vec<u8>, StorageError> {
        let mut buf = Vec::new();
        self.encode_value(value, &mut buf, true)?;
        Ok(buf)
    }

    /// Decode a single `Value` from bytes produced by [`encode`].
    pub fn decode(&self, bytes: &[u8]) -> Result<Value, StorageError> {
        let (value, consumed) = self.decode_value_inner(bytes)?;
        if consumed != bytes.len() {
            return Err(StorageError::deserialize_error(format!(
                "OrderedCodec decode: {} bytes trailing data after value",
                bytes.len() - consumed
            )));
        }
        Ok(value)
    }

    /// Decode the first value; return the value and bytes consumed.
    pub fn decode_value_inner(&self, bytes: &[u8]) -> Result<(Value, usize), StorageError> {
        if bytes.is_empty() {
            return Err(StorageError::deserialize_error(
                "OrderedCodec decode_value: empty input",
            ));
        }
        let tag = bytes[0];
        match tag {
            TAG_NULL | TAG_NULL_LAST => Ok((Value::Null(NullType::Null), 1)),
            TAG_BOOL => {
                if bytes.len() < 2 {
                    return Err(StorageError::deserialize_error("truncated bool"));
                }
                Ok((Value::Bool(bytes[1] != 0), 2))
            }
            TAG_SMALL_INT => {
                if bytes.len() < 3 {
                    return Err(StorageError::deserialize_error("truncated smallint"));
                }
                let raw = u16::from_be_bytes([bytes[1], bytes[2]]);
                Ok((Value::SmallInt((raw ^ 0x8000) as i16), 3))
            }
            TAG_INT => {
                if bytes.len() < 5 {
                    return Err(StorageError::deserialize_error("truncated int"));
                }
                let raw = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                Ok((Value::Int((raw ^ 0x8000_0000) as i32), 5))
            }
            TAG_BIG_INT => {
                if bytes.len() < 9 {
                    return Err(StorageError::deserialize_error("truncated bigint"));
                }
                let raw = u64::from_be_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                Ok((Value::BigInt((raw ^ 0x8000_0000_0000_0000) as i64), 9))
            }
            TAG_FLOAT => {
                if bytes.len() < 5 {
                    return Err(StorageError::deserialize_error("truncated float"));
                }
                let raw = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                Ok((Value::Float(decode_f32_ordered(raw)), 5))
            }
            TAG_DOUBLE => {
                if bytes.len() < 9 {
                    return Err(StorageError::deserialize_error("truncated double"));
                }
                let raw = u64::from_be_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                Ok((Value::Double(decode_f64_ordered(raw)), 9))
            }
            TAG_STRING | TAG_BLOB => {
                // Terminator-based: scan for 0x00 byte (the terminator).
                let term_rel = bytes[1..].iter().position(|b| *b == 0).ok_or_else(|| {
                    StorageError::deserialize_error("missing string/blob terminator")
                })?;
                let term_abs = 1 + term_rel;
                let data = bytes[1..term_abs].to_vec();
                let end = term_abs + 1;
                if tag == TAG_STRING {
                    let s = String::from_utf8(data).map_err(|e| {
                        StorageError::deserialize_error(format!("invalid UTF-8: {}", e))
                    })?;
                    Ok((Value::String(s), end))
                } else {
                    Ok((Value::Blob(data), end))
                }
            }
            TAG_DATE => {
                if bytes.len() < 7 {
                    return Err(StorageError::deserialize_error("truncated date"));
                }
                let raw_year = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let year = (raw_year ^ 0x8000_0000) as i32;
                let month = bytes[5] as u32;
                let day = bytes[6] as u32;
                Ok((Value::Date(DateValue { year, month, day }), 7))
            }
            TAG_TIME => {
                if bytes.len() < 9 {
                    return Err(StorageError::deserialize_error("truncated time"));
                }
                let total_micros = u64::from_be_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                let microsec = (total_micros % 1_000_000) as u32;
                let total_secs = total_micros / 1_000_000;
                let hour = (total_secs / 3600) as u32;
                let minute = ((total_secs % 3600) / 60) as u32;
                let sec = (total_secs % 60) as u32;
                Ok((
                    Value::Time(TimeValue {
                        hour,
                        minute,
                        sec,
                        microsec,
                    }),
                    9,
                ))
            }
            TAG_DATE_TIME => {
                if bytes.len() < 15 {
                    return Err(StorageError::deserialize_error("truncated datetime"));
                }
                let raw_year = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                let year = (raw_year ^ 0x8000_0000) as i32;
                let month = bytes[5] as u32;
                let day = bytes[6] as u32;
                let total_micros = u64::from_be_bytes([
                    bytes[7], bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13],
                    bytes[14],
                ]);
                let microsec = (total_micros % 1_000_000) as u32;
                let total_secs = total_micros / 1_000_000;
                let hour = (total_secs / 3600) as u32;
                let minute = ((total_secs % 3600) / 60) as u32;
                let sec = (total_secs % 60) as u32;
                Ok((
                    Value::DateTime(DateTimeValue {
                        year,
                        month,
                        day,
                        hour,
                        minute,
                        sec,
                        microsec,
                    }),
                    15,
                ))
            }
            TAG_UUID => {
                if bytes.len() < 17 {
                    return Err(StorageError::deserialize_error("truncated uuid"));
                }
                let mut uuid_bytes = [0u8; 16];
                uuid_bytes.copy_from_slice(&bytes[1..17]);
                Ok((Value::Uuid(UuidValue(uuid_bytes)), 17))
            }
            TAG_VERTEX_ID => {
                let (entity, consumed) = self.decode_entity_bytes(bytes)?;
                match entity {
                    EntityRef::Vertex(vid) => {
                        use std::collections::HashMap;
                        let val = Value::Vertex(Box::new(crate::core::Vertex {
                            vid,
                            id: 0,
                            tags: Vec::new(),
                            properties: HashMap::new(),
                        }));
                        Ok((val, consumed))
                    }
                    EntityRef::Edge { .. } => Err(StorageError::deserialize_error(
                        "expected vertex tag but found edge ref",
                    )),
                }
            }
            TAG_EDGE_REF => {
                let (_entity, consumed) = self.decode_entity_bytes(bytes)?;
                // Edge refs are rare as index key values; encode as debug string
                Ok((Value::String("(edge)".to_string()), consumed))
            }
            _ => Err(StorageError::deserialize_error(format!(
                "unknown type tag 0x{:02x}",
                tag
            ))),
        }
    }

    /// Encode a composite index key from multiple values and an optional entity
    /// tie-breaker.
    pub fn encode_composite(
        &self,
        values: &[&Value],
        entity: Option<&EntityRef>,
        null_last: bool,
    ) -> Result<Vec<u8>, StorageError> {
        let mut buf = Vec::new();
        for v in values {
            self.encode_value(v, &mut buf, null_last)?;
        }
        if let Some(e) = entity {
            self.encode_entity(e, &mut buf)?;
        }
        Ok(buf)
    }

    /// Encode an entity reference (Vertex or Edge) as a tie-breaker.
    pub fn encode_entity(&self, entity: &EntityRef, buf: &mut Vec<u8>) -> Result<(), StorageError> {
        match entity {
            EntityRef::Vertex(vid) => {
                buf.push(TAG_VERTEX_ID);
                write_vertex_id(vid, buf);
                Ok(())
            }
            EntityRef::Edge {
                src,
                dst,
                edge_type,
                ranking,
            } => {
                buf.push(TAG_EDGE_REF);
                write_vertex_id(src, buf);
                write_vertex_id(dst, buf);
                buf.extend_from_slice(&edge_type.to_be_bytes());
                buf.extend_from_slice(&ranking.to_be_bytes());
                Ok(())
            }
        }
    }

    /// Decode an entity reference from bytes.
    pub fn decode_entity_bytes(&self, bytes: &[u8]) -> Result<(EntityRef, usize), StorageError> {
        if bytes.is_empty() {
            return Err(StorageError::deserialize_error("empty entity input"));
        }
        let tag = bytes[0];
        match tag {
            TAG_VERTEX_ID => {
                let (vid, consumed) = read_vertex_id(&bytes[1..])?;
                Ok((EntityRef::Vertex(vid), 1 + consumed))
            }
            TAG_EDGE_REF => {
                let mut pos = 1;
                let (src, c1) = read_vertex_id(&bytes[pos..])?;
                pos += c1;
                let (dst, c2) = read_vertex_id(&bytes[pos..])?;
                pos += c2;
                if bytes.len() < pos + 12 {
                    return Err(StorageError::deserialize_error("truncated edge ref"));
                }
                let et_bytes: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
                let rank_bytes: [u8; 8] = bytes[pos + 4..pos + 12].try_into().unwrap();
                pos += 12;
                Ok((
                    EntityRef::Edge {
                        src,
                        dst,
                        edge_type: u32::from_be_bytes(et_bytes),
                        ranking: i64::from_be_bytes(rank_bytes),
                    },
                    pos,
                ))
            }
            _ => Err(StorageError::deserialize_error(format!(
                "unknown entity tag 0x{:02x}",
                tag
            ))),
        }
    }

    /// Compute the prefix upper bound for range scanning.
    ///
    /// Increments the last byte of the prefix (with carry) to produce an
    /// exclusive upper bound for a tight prefix range.
    pub fn prefix_upper_bound(prefix: &[u8]) -> Vec<u8> {
        let mut end = prefix.to_vec();
        for i in (0..end.len()).rev() {
            if end[i] == 0xFF {
                end[i] = 0x00;
            } else {
                end[i] += 1;
                break;
            }
        }
        end
    }

    /// Return tight (inclusive lower, exclusive upper) byte bounds for a
    /// prefix scan over the given value.
    ///
    /// For fixed-length types the bounds are the encoded value and its
    /// [`prefix_upper_bound`].  For variable-length types (String, Blob)
    /// the lower bound is the full encoded value (including terminator)
    /// and the upper bound is the tag followed by
    /// `prefix_upper_bound(data_bytes)` — this correctly covers all values
    /// whose encoded prefix equals the given value's encoding while
    /// excluding the next value in order.
    pub fn prefix_bounds(&self, value: &Value) -> Result<(Vec<u8>, Vec<u8>), StorageError> {
        match value {
            // Fixed-length types: simple upper bound from the encoded form.
            Value::Null(_)
            | Value::Empty
            | Value::Bool(_)
            | Value::SmallInt(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Double(_)
            | Value::Date(_)
            | Value::Time(_)
            | Value::DateTime(_)
            | Value::Uuid(_) => {
                let lower = self.encode(value)?;
                let upper = Self::prefix_upper_bound(&lower);
                Ok((lower, upper))
            }

            // String / blob: terminator-based, tight upper bound.
            // Lower = full encoded value (TAG + data + 0x00).
            // Upper = TAG + prefix_upper_bound(data).
            Value::String(_s) => {
                let lower = self.encode(value)?;
                let tag = lower[0];
                let data = &lower[1..lower.len() - 1]; // strip tag and terminator
                let mut upper = vec![tag];
                upper.extend_from_slice(&Self::prefix_upper_bound(data));
                Ok((lower, upper))
            }
            Value::FixedString { data, .. } => {
                let s = Value::String(data.clone());
                let lower = self.encode(&s)?;
                let tag = lower[0];
                let data_bytes = &lower[1..lower.len() - 1];
                let mut upper = vec![tag];
                upper.extend_from_slice(&Self::prefix_upper_bound(data_bytes));
                Ok((lower, upper))
            }
            Value::Blob(_b) => {
                let lower = self.encode(value)?;
                let tag = lower[0];
                let data = &lower[1..lower.len() - 1];
                let mut upper = vec![tag];
                upper.extend_from_slice(&Self::prefix_upper_bound(data));
                Ok((lower, upper))
            }

            // Complex / non-indexable types: error.
            other => Err(StorageError::db_error(format!(
                "prefix_bounds not supported for type {:?}",
                other
            ))),
        }
    }

    // ── Internal encoding ────────────────────────────────────────────────────

    fn encode_value(
        &self,
        value: &Value,
        buf: &mut Vec<u8>,
        null_last: bool,
    ) -> Result<(), StorageError> {
        match value {
            Value::Empty | Value::Null(_) => {
                buf.push(if null_last { TAG_NULL_LAST } else { TAG_NULL });
            }
            Value::Bool(b) => {
                buf.push(TAG_BOOL);
                buf.push(u8::from(*b));
            }
            Value::SmallInt(n) => {
                buf.push(TAG_SMALL_INT);
                buf.extend_from_slice(&((*n as u16) ^ 0x8000).to_be_bytes());
            }
            Value::Int(n) => {
                buf.push(TAG_INT);
                buf.extend_from_slice(&((*n as u32) ^ 0x8000_0000).to_be_bytes());
            }
            Value::BigInt(n) => {
                buf.push(TAG_BIG_INT);
                buf.extend_from_slice(&((*n as u64) ^ 0x8000_0000_0000_0000).to_be_bytes());
            }
            Value::Float(f) => {
                buf.push(TAG_FLOAT);
                buf.extend_from_slice(&encode_f32_ordered(*f).to_be_bytes());
            }
            Value::Double(f) => {
                buf.push(TAG_DOUBLE);
                buf.extend_from_slice(&encode_f64_ordered(*f).to_be_bytes());
            }
            Value::String(s) => {
                if s.as_bytes().contains(&0) {
                    return Err(StorageError::db_error(
                        "string with embedded 0x00 byte cannot be encoded in ordered_codec v1",
                    ));
                }
                buf.push(TAG_STRING);
                buf.extend_from_slice(s.as_bytes());
                buf.push(0x00);
            }
            Value::FixedString { data, .. } => {
                if data.as_bytes().contains(&0) {
                    return Err(StorageError::db_error(
                        "fixed string with embedded 0x00 byte cannot be encoded",
                    ));
                }
                buf.push(TAG_STRING);
                buf.extend_from_slice(data.as_bytes());
                buf.push(0x00);
            }
            Value::Blob(b) => {
                if b.contains(&0) {
                    return Err(StorageError::db_error(
                        "blob with embedded 0x00 byte cannot be encoded in ordered_codec v1",
                    ));
                }
                buf.push(TAG_BLOB);
                buf.extend_from_slice(b);
                buf.push(0x00);
            }
            Value::Date(d) => {
                buf.push(TAG_DATE);
                buf.extend_from_slice(&((d.year as u32) ^ 0x8000_0000).to_be_bytes());
                buf.push(d.month as u8);
                buf.push(d.day as u8);
            }
            Value::Time(t) => {
                buf.push(TAG_TIME);
                let total_micros = (t.hour as u64) * 3_600_000_000
                    + (t.minute as u64) * 60_000_000
                    + (t.sec as u64) * 1_000_000
                    + t.microsec as u64;
                buf.extend_from_slice(&total_micros.to_be_bytes());
            }
            Value::DateTime(dt) => {
                buf.push(TAG_DATE_TIME);
                buf.extend_from_slice(&((dt.year as u32) ^ 0x8000_0000).to_be_bytes());
                buf.push(dt.month as u8);
                buf.push(dt.day as u8);
                let total_micros = (dt.hour as u64) * 3_600_000_000
                    + (dt.minute as u64) * 60_000_000
                    + (dt.sec as u64) * 1_000_000
                    + dt.microsec as u64;
                buf.extend_from_slice(&total_micros.to_be_bytes());
            }
            Value::Uuid(u) => {
                buf.push(TAG_UUID);
                buf.extend_from_slice(&u.0);
            }
            // Complex types: encode as null (these shouldn't be index keys)
            Value::Decimal128(_)
            | Value::Vertex(_)
            | Value::Edge(_)
            | Value::Path(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::Geography(_)
            | Value::Vector(_)
            | Value::DataSet(_)
            | Value::Json(_)
            | Value::JsonB(_)
            | Value::Interval(_) => {
                buf.push(TAG_NULL);
            }
        }
        Ok(())
    }
}

impl Default for OrderedCodec {
    fn default() -> Self {
        Self::new()
    }
}

// ── VertexId I/O ────────────────────────────────────────────────────────────

/// Write a VertexId as fixed-size 16 bytes for order-preserving comparison.
/// Format: [kind:1] [len:1] [data:14] (padded with zeros).
fn write_vertex_id(vid: &VertexId, buf: &mut Vec<u8>) {
    let bytes = vid.as_bytes();
    let kind: u8 = if vid.is_int64() { 0 } else { 1 };
    buf.push(kind);
    buf.push(bytes.len() as u8);
    buf.extend_from_slice(bytes);
    let pad = 14usize.saturating_sub(bytes.len());
    buf.extend(std::iter::repeat_n(0u8, pad));
}

/// Read a VertexId from 16 fixed-size bytes.
fn read_vertex_id(bytes: &[u8]) -> Result<(VertexId, usize), StorageError> {
    if bytes.len() < 16 {
        return Err(StorageError::deserialize_error(
            "truncated vertex id (need 16)",
        ));
    }
    let _kind = bytes[0];
    let len = bytes[1] as usize;
    let data = &bytes[2..2 + len.min(14)];
    if _kind == 0 {
        // i64: first 8 bytes of data
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&data[..data.len().min(8)]);
        let val = i64::from_be_bytes(arr);
        Ok((VertexId::from_int64(val), 16))
    } else {
        let s = std::str::from_utf8(data).map_err(|e| {
            StorageError::deserialize_error(format!("invalid vertex id UTF-8: {}", e))
        })?;
        Ok((VertexId::from_string(s.to_string()), 16))
    }
}

// ── IEEE 754 total-order encoding ───────────────────────────────────────────

fn encode_f32_ordered(f: f32) -> u32 {
    let bits = f.to_bits();
    if f.is_sign_negative() {
        !bits
    } else {
        bits ^ 0x8000_0000
    }
}

fn decode_f32_ordered(bits: u32) -> f32 {
    if bits & 0x8000_0000 != 0 {
        f32::from_bits(bits ^ 0x8000_0000)
    } else {
        f32::from_bits(!bits)
    }
}

fn encode_f64_ordered(f: f64) -> u64 {
    let bits = f.to_bits();
    if f.is_sign_negative() {
        !bits
    } else {
        bits ^ 0x8000_0000_0000_0000
    }
}

fn decode_f64_ordered(bits: u64) -> f64 {
    if bits & 0x8000_0000_0000_0000 != 0 {
        f64::from_bits(bits ^ 0x8000_0000_0000_0000)
    } else {
        f64::from_bits(!bits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::date_time::{DateTimeValue, DateValue, TimeValue};
    use crate::core::value::UuidValue;

    fn codec() -> OrderedCodec {
        OrderedCodec::new()
    }

    #[test]
    fn test_null() {
        let v = Value::Null(NullType::Null);
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn test_empty() {
        let v = Value::Empty;
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, Value::Null(NullType::Null));
    }

    #[test]
    fn test_bool() {
        for v in &[Value::Bool(false), Value::Bool(true)] {
            let enc = codec().encode(v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, *v);
        }
    }

    #[test]
    fn test_bool_order() {
        let f = codec().encode(&Value::Bool(false)).unwrap();
        let t = codec().encode(&Value::Bool(true)).unwrap();
        assert!(f < t, "false < true");
    }

    #[test]
    fn test_ints() {
        let cases = [
            Value::SmallInt(i16::MIN),
            Value::SmallInt(-1),
            Value::SmallInt(0),
            Value::SmallInt(42),
            Value::SmallInt(i16::MAX),
            Value::Int(i32::MIN),
            Value::Int(-1),
            Value::Int(0),
            Value::Int(42),
            Value::Int(i32::MAX),
            Value::BigInt(i64::MIN),
            Value::BigInt(-1),
            Value::BigInt(0),
            Value::BigInt(42),
            Value::BigInt(i64::MAX),
        ];
        for v in &cases {
            let enc = codec().encode(v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, *v, "roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_int_order() {
        let neg = codec().encode(&Value::Int(-5)).unwrap();
        let zero = codec().encode(&Value::Int(0)).unwrap();
        let pos = codec().encode(&Value::Int(5)).unwrap();
        assert!(neg < zero, "-5 < 0");
        assert!(zero < pos, "0 < 5");
    }

    #[test]
    fn test_float() {
        let cases = [
            Value::Float(f32::NEG_INFINITY),
            Value::Float(-1.0),
            Value::Float(-0.0),
            Value::Float(0.0),
            Value::Float(1.0),
            Value::Float(f32::INFINITY),
            Value::Double(f64::NEG_INFINITY),
            Value::Double(-1.0),
            Value::Double(-0.0),
            Value::Double(0.0),
            Value::Double(1.0),
            Value::Double(f64::INFINITY),
        ];
        for v in &cases {
            let enc = codec().encode(v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, *v, "roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_float_order() {
        let neg = codec().encode(&Value::Float(-1.0)).unwrap();
        let zero = codec().encode(&Value::Float(0.0)).unwrap();
        let pos = codec().encode(&Value::Float(1.0)).unwrap();
        assert!(neg < zero, "-1 < 0");
        assert!(zero < pos, "0 < 1");
    }

    #[test]
    fn test_string_roundtrip() {
        let cases = [
            Value::String("".to_string()),
            Value::String("hello".to_string()),
            Value::String("世界".to_string()),
            Value::FixedString {
                len: 5,
                data: "fixed".to_string(),
            },
        ];
        for v in &cases {
            let enc = codec().encode(v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, v.fixed_to_string_value());
        }
    }

    #[test]
    fn test_string_order_aa_less_than_b() {
        let aa = codec().encode(&Value::String("aa".to_string())).unwrap();
        let b = codec().encode(&Value::String("b".to_string())).unwrap();
        assert!(
            aa < b,
            "encoded('aa') < encoded('b') must hold for semantic ordering"
        );
    }

    #[test]
    fn test_string_order() {
        let a = codec().encode(&Value::String("a".to_string())).unwrap();
        let b = codec().encode(&Value::String("b".to_string())).unwrap();
        assert!(a < b);
    }

    #[test]
    fn test_blob_roundtrip() {
        let cases = vec![
            Value::Blob(vec![]),
            Value::Blob(vec![0x01, 0x02, 0x03]),
            Value::Blob(vec![0xFF, 0xFE]),
        ];
        // Note: blobs with 0x00 byte are rejected at encode time.
        for v in cases {
            let enc = codec().encode(&v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, v, "roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_blob_rejects_zero_byte() {
        let v = Value::Blob(vec![0x00, 0x01]);
        assert!(codec().encode(&v).is_err());
    }

    #[test]
    fn test_string_rejects_zero_byte() {
        let v = Value::String("a\x00b".to_string());
        assert!(codec().encode(&v).is_err());
    }

    #[test]
    fn test_prefix_bounds_string() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::String("a".to_string()))
            .unwrap();
        // lower = [TAG_STRING, 0x61, 0x00]
        assert_eq!(lower[0], TAG_STRING);
        assert_eq!(&lower[1..lower.len() - 1], b"a");
        assert_eq!(lower[lower.len() - 1], 0x00);

        // upper = [TAG_STRING, 0x62]
        assert_eq!(upper, vec![TAG_STRING, 0x62]);

        // Verify all "a*" strings fall within bounds
        for suffix in &["", "a", "aa", "az"] {
            let s_val = format!("a{}", suffix);
            let enc = codec().encode(&Value::String(s_val.clone())).unwrap();
            assert!(enc.as_slice() >= lower.as_slice(), "{:?} >= lower", s_val);
            assert!(enc.as_slice() < upper.as_slice(), "{:?} < upper", s_val);
        }

        // "b" is excluded
        let b_enc = codec().encode(&Value::String("b".to_string())).unwrap();
        assert!(b_enc.as_slice() >= upper.as_slice());
    }

    #[test]
    fn test_prefix_bounds_multi_byte() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::String("ab".to_string()))
            .unwrap();
        assert_eq!(&lower[1..lower.len() - 1], b"ab");
        assert_eq!(upper, vec![TAG_STRING, 0x61, 0x63]);

        let aba = codec().encode(&Value::String("aba".to_string())).unwrap();
        assert!(aba.as_slice() < upper.as_slice());

        let ac = codec().encode(&Value::String("ac".to_string())).unwrap();
        assert!(ac.as_slice() >= upper.as_slice());
    }

    #[test]
    fn test_prefix_bounds_blob() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::Blob(vec![0x01, 0x02]))
            .unwrap();
        assert_eq!(lower[0], TAG_BLOB);
        assert_eq!(&lower[1..lower.len() - 1], &[0x01, 0x02]);
        assert_eq!(upper, vec![TAG_BLOB, 0x01, 0x03]);
    }

    #[test]
    fn test_prefix_bounds_int() {
        let (lower, upper) = codec().prefix_bounds(&Value::Int(42)).unwrap();
        assert!(lower < upper);
        // No other int with bytes starting with 42 exists (fixed-length),
        // so upper should follow right after lower
        let forty_two = codec().encode(&Value::Int(42)).unwrap();
        assert_eq!(lower, forty_two);
    }

    #[test]
    fn test_encoded_order_matches_semantic_order() {
        // Verify that ordering is preserved for pairs that the length-prefixed
        // encoding got wrong.
        let pairs: [(Value, Value); 4] = [
            (Value::String("aa".into()), Value::String("b".into())),
            (Value::String("a".into()), Value::String("aa".into())),
            (Value::String("".into()), Value::String("a".into())),
            (Value::String("a".into()), Value::String("ab".into())),
        ];
        for (a, b) in &pairs {
            let enc_a = codec().encode(a).unwrap();
            let enc_b = codec().encode(b).unwrap();
            assert!(
                enc_a < enc_b,
                "ordered_codec: {:?} < {:?} should hold, but enc({:?}) >= enc({:?})",
                a,
                b,
                a,
                b
            );
        }
    }

    #[test]
    fn test_prefix_bounds_overflow() {
        // Prefix with last byte = 0xFF should carry correctly
        let (lower, upper) = codec()
            .prefix_bounds(&Value::Blob(vec![0x01, 0xFF]))
            .unwrap();
        assert_eq!(&lower[1..lower.len() - 1], &[0x01, 0xFF]);
        // prefix_upper_bound([0x01, 0xFF]) = [0x02, 0x00]
        assert_eq!(upper, vec![TAG_BLOB, 0x02, 0x00]);

        // Blob starting with [0x01, 0xFF] should be within bounds
        let test = codec()
            .encode(&Value::Blob(vec![0x01, 0xFF, 0x01]))
            .unwrap();
        assert!(test.as_slice() < upper.as_slice(), "0x01,0xFF,0x01 < upper");
        // Blob starting with [0x02] should be outside bounds
        let next = codec().encode(&Value::Blob(vec![0x02])).unwrap();
        assert!(next.as_slice() >= upper.as_slice(), "0x02 >= upper");
    }

    #[test]
    fn test_uuid() {
        let u = Value::Uuid(UuidValue::from_bytes([
            0x55, 0x0E, 0x84, 0x00, 0xE2, 0x9B, 0x41, 0xD4, 0xA7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ]));
        let enc = codec().encode(&u).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, u);
    }

    #[test]
    fn test_date() {
        let v = Value::Date(DateValue {
            year: 2024,
            month: 6,
            day: 15,
        });
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn test_time() {
        let v = Value::Time(TimeValue {
            hour: 14,
            minute: 30,
            sec: 0,
            microsec: 500_000,
        });
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn test_datetime() {
        let v = Value::DateTime(DateTimeValue {
            year: 2024,
            month: 6,
            day: 15,
            hour: 14,
            minute: 30,
            sec: 0,
            microsec: 500,
        });
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn test_composite_key() {
        let v1 = Value::String("name".to_string());
        let v2 = Value::Int(42);
        let vid = VertexId::from_int64(123);
        let entity = EntityRef::Vertex(vid);

        let key = codec()
            .encode_composite(&[&v1, &v2], Some(&entity), false)
            .unwrap();
        assert!(!key.is_empty());

        let (dv1, c1) = codec().decode_value_inner(&key).unwrap();
        assert_eq!(dv1, v1);
        let (dv2, _c2) = codec().decode_value_inner(&key[c1..]).unwrap();
        assert_eq!(dv2, v2);
    }

    #[test]
    fn test_composite_order() {
        let pre = Value::String("a".to_string());
        let k1 = codec()
            .encode_composite(&[&pre, &Value::Int(1)], None, false)
            .unwrap();
        let k2 = codec()
            .encode_composite(&[&pre, &Value::Int(2)], None, false)
            .unwrap();
        assert!(k1 < k2);

        let k3 = codec()
            .encode_composite(
                &[&Value::String("b".to_string()), &Value::Int(0)],
                None,
                false,
            )
            .unwrap();
        assert!(k1 < k3, "'a' prefix < 'b' prefix");
    }

    #[test]
    fn test_prefix_upper_bound() {
        assert_eq!(OrderedCodec::prefix_upper_bound(&[1, 2, 3]), vec![1, 2, 4]);
        assert_eq!(OrderedCodec::prefix_upper_bound(&[1, 255]), vec![2, 0]);
        assert_eq!(
            OrderedCodec::prefix_upper_bound(&[255, 255, 255]),
            vec![0, 0, 0]
        );
    }

    #[test]
    fn test_vertex_id_tie_breaker() {
        let vid = VertexId::from_int64(42);
        let v = Value::String("hello".to_string());
        let key = codec()
            .encode_composite(&[&v], Some(&EntityRef::Vertex(vid)), false)
            .unwrap();
        let (dec, _c) = codec().decode_value_inner(&key).unwrap();
        assert_eq!(dec, v);
    }

    /// Helper: FixedString → Value::String for comparison
    trait FixedToStr {
        fn fixed_to_string_value(&self) -> Value;
    }
    impl FixedToStr for Value {
        fn fixed_to_string_value(&self) -> Value {
            match self {
                Value::FixedString { data, .. } => Value::String(data.clone()),
                other => other.clone(),
            }
        }
    }
}

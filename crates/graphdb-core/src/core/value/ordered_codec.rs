//! Ordered key codec — order-preserving binary encoding for `Value`.
//!
//! The byte-level comparison of encoded values matches the semantic ordering
//! of values within the same index type. Distinct types are ordered by their
//! stable type tags. This enables range and prefix scans on index keys without
//! post-filtering while preserving the original value type in the key format.
//!
//! # Format (version 2)
//!
//! Each field starts with a type tag byte, followed by the encoded body.
//! Fixed-length types encode to a fixed byte count; variable-length types
//! (String, Blob) use an escaped 0x00 terminator. A zero byte in the data is
//! encoded as `00 01`; the terminator is `00 00`.
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
use compact_str::CompactString;

/// Current wire format version.
pub const ORDERED_CODEC_VERSION: u8 = 0x02;

// ── Type tags (1 byte) ──────────────────────────────────────────────────────

const TAG_EMPTY: u8 = 0x00;
const TAG_NULL: u8 = 0x01;
const TAG_BOOL: u8 = 0x02;
const TAG_SMALL_INT: u8 = 0x03;
const TAG_INT: u8 = 0x04;
const TAG_BIG_INT: u8 = 0x05;
const TAG_FLOAT: u8 = 0x06;
const TAG_DOUBLE: u8 = 0x07;
const TAG_DECIMAL: u8 = 0x08;
const TAG_DATE: u8 = 0x09;
const TAG_TIME: u8 = 0x0A;
const TAG_DATE_TIME: u8 = 0x0B;
const TAG_STRING: u8 = 0x0C;
const TAG_FIXED_STRING: u8 = 0x0D;
const TAG_BLOB: u8 = 0x0E;
const TAG_VERTEX_ID: u8 = 0x0F;
const TAG_EDGE_REF: u8 = 0x10;
const TAG_UUID: u8 = 0x1A;
const TAG_NULL_LAST: u8 = 0xFF;
const NULL_KIND_EMPTY: u8 = 0xFF;

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
            TAG_EMPTY => Ok((Value::Empty, 1)),
            TAG_NULL | TAG_NULL_LAST => {
                if bytes.len() < 2 {
                    return Err(StorageError::deserialize_error("truncated null value"));
                }
                let kind = bytes[1];
                if tag == TAG_NULL_LAST && kind == NULL_KIND_EMPTY {
                    Ok((Value::Empty, 2))
                } else {
                    Ok((Value::Null(decode_null_type(kind)?), 2))
                }
            }
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
            TAG_DECIMAL => {
                let (value, consumed) = decode_decimal(bytes)?;
                Ok((value, consumed))
            }
            TAG_STRING | TAG_FIXED_STRING | TAG_BLOB => {
                let (data, end) = decode_escaped_bytes(bytes, 1)?;
                if tag == TAG_STRING {
                    let s = String::from_utf8(data).map_err(|e| {
                        StorageError::deserialize_error(format!("invalid UTF-8: {}", e))
                    })?;
                    Ok((Value::String(CompactString::from(s)), end))
                } else if tag == TAG_BLOB {
                    Ok((Value::Blob(data), end))
                } else {
                    if bytes.len() < end + 4 {
                        return Err(StorageError::deserialize_error(
                            "truncated fixed-string length",
                        ));
                    }
                    let len = u32::from_be_bytes([
                        bytes[end],
                        bytes[end + 1],
                        bytes[end + 2],
                        bytes[end + 3],
                    ]) as usize;
                    let s = String::from_utf8(data).map_err(|e| {
                        StorageError::deserialize_error(format!("invalid UTF-8: {}", e))
                    })?;
                    Ok((Value::FixedString { len, data: s }, end + 4))
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
                Ok((Value::String(CompactString::new("(edge)")), consumed))
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
                let et_bytes: [u8; 4] = bytes[pos..pos + 4]
                    .try_into()
                    .map_err(|_| StorageError::deserialize_error("invalid edge type bytes"))?;
                let rank_bytes: [u8; 8] = bytes[pos + 4..pos + 12]
                    .try_into()
                    .map_err(|_| StorageError::deserialize_error("invalid edge ranking bytes"))?;
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

            // String / fixed string / blob: escaped-terminator based, tight
            // upper bound. The length suffix of a fixed string is deliberately
            // outside the prefix range so all values with the requested data
            // prefix remain covered.
            // Lower = full encoded value (TAG + data + 0x00).
            // Upper = TAG + prefix_upper_bound(data).
            Value::String(_) | Value::FixedString { .. } | Value::Blob(_) => {
                let lower = self.encode(value)?;
                let tag = lower[0];
                let (_, terminator_end) = decode_escaped_bytes(&lower, 1)?;
                let data = &lower[1..terminator_end - 2];
                let mut upper = vec![tag];
                if data.is_empty() {
                    upper = Self::prefix_upper_bound(&upper);
                } else {
                    upper.extend_from_slice(&Self::prefix_upper_bound(data));
                }
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
            Value::Empty => {
                if null_last {
                    buf.extend_from_slice(&[TAG_NULL_LAST, NULL_KIND_EMPTY]);
                } else {
                    buf.push(TAG_EMPTY);
                }
            }
            Value::Null(null_type) => {
                buf.push(if null_last { TAG_NULL_LAST } else { TAG_NULL });
                buf.push(encode_null_type(null_type));
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
            Value::Decimal128(decimal) => {
                encode_decimal(decimal, buf)?;
            }
            Value::String(s) => {
                buf.push(TAG_STRING);
                encode_escaped_bytes(s.as_bytes(), buf);
            }
            Value::FixedString { len, data } => {
                buf.push(TAG_FIXED_STRING);
                encode_escaped_bytes(data.as_bytes(), buf);
                buf.extend_from_slice(&(*len as u32).to_be_bytes());
            }
            Value::Blob(b) => {
                buf.push(TAG_BLOB);
                encode_escaped_bytes(b, buf);
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
            // Complex values are not valid ordered index fields. Silently
            // encoding them as NULL would create false index matches.
            Value::Vertex(_)
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
                return Err(StorageError::invalid_input(format!(
                    "Value type {:?} is not supported by OrderedCodec",
                    value.get_type()
                )));
            }
        }
        Ok(())
    }
}

fn encode_escaped_bytes(data: &[u8], buf: &mut Vec<u8>) {
    for byte in data {
        if *byte == 0 {
            buf.extend_from_slice(&[0, 1]);
        } else {
            buf.push(*byte);
        }
    }
    buf.extend_from_slice(&[0, 0]);
}

fn encode_null_type(null_type: &NullType) -> u8 {
    match null_type {
        NullType::Null => 0,
        NullType::NaN => 1,
        NullType::BadData => 2,
        NullType::BadType => 3,
        NullType::ErrOverflow => 4,
        NullType::UnknownProp => 5,
        NullType::DivByZero => 6,
        NullType::OutOfRange => 7,
    }
}

fn decode_null_type(code: u8) -> Result<NullType, StorageError> {
    match code {
        0 => Ok(NullType::Null),
        1 => Ok(NullType::NaN),
        2 => Ok(NullType::BadData),
        3 => Ok(NullType::BadType),
        4 => Ok(NullType::ErrOverflow),
        5 => Ok(NullType::UnknownProp),
        6 => Ok(NullType::DivByZero),
        7 => Ok(NullType::OutOfRange),
        _ => Err(StorageError::deserialize_error(format!(
            "invalid null type code {code}"
        ))),
    }
}

fn decode_escaped_bytes(
    bytes: &[u8],
    mut position: usize,
) -> Result<(Vec<u8>, usize), StorageError> {
    let mut data = Vec::new();
    while position < bytes.len() {
        let byte = bytes[position];
        position += 1;
        if byte != 0 {
            data.push(byte);
            continue;
        }
        let escape = *bytes.get(position).ok_or_else(|| {
            StorageError::deserialize_error("missing variable-length value terminator")
        })?;
        position += 1;
        match escape {
            0 => return Ok((data, position)),
            1 => data.push(0),
            _ => {
                return Err(StorageError::deserialize_error(
                    "invalid variable-length value escape",
                ))
            }
        }
    }
    Err(StorageError::deserialize_error(
        "missing variable-length value terminator",
    ))
}

fn encode_decimal(
    decimal: &crate::core::value::Decimal128Value,
    buf: &mut Vec<u8>,
) -> Result<(), StorageError> {
    let text = decimal.to_string();
    let negative = text.starts_with('-');
    let unsigned = text
        .strip_prefix('-')
        .or_else(|| text.strip_prefix('+'))
        .unwrap_or(&text);
    let (mantissa, exponent_part) = unsigned
        .find(['e', 'E'])
        .map_or((unsigned, "0"), |position| {
            (&unsigned[..position], &unsigned[position + 1..])
        });
    let exponent10 = exponent_part.parse::<i32>().map_err(|error| {
        StorageError::serialize_error(format!("Invalid decimal exponent: {error}"))
    })?;
    let (whole, fractional) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |parts| parts);
    let mut digits = whole.bytes().chain(fractional.bytes()).collect::<Vec<_>>();
    if digits.is_empty() || digits.iter().any(|digit| !digit.is_ascii_digit()) {
        return Err(StorageError::serialize_error(format!(
            "Invalid decimal value {text}"
        )));
    }
    let is_zero = digits.iter().all(|digit| *digit == b'0');
    let (digit_exponent, negative) = if is_zero {
        digits.clear();
        digits.push(b'0');
        (i32::MIN, false)
    } else {
        let leading_zero_count = digits.iter().take_while(|digit| **digit == b'0').count();
        let whole_len = i32::try_from(whole.len())
            .map_err(|_| StorageError::serialize_error("Decimal integral part is too long"))?;
        let leading_zero_count = i32::try_from(leading_zero_count)
            .map_err(|_| StorageError::serialize_error("Decimal significand is too long"))?;
        let digit_exponent = whole_len
            .checked_add(exponent10)
            .and_then(|value| value.checked_sub(leading_zero_count))
            .ok_or_else(|| StorageError::serialize_error("Decimal exponent is out of range"))?;

        digits.drain(..leading_zero_count as usize);
        while digits.last() == Some(&b'0') {
            digits.pop();
        }
        (digit_exponent, negative)
    };
    let biased = (digit_exponent as u32) ^ 0x8000_0000;
    buf.push(TAG_DECIMAL);
    buf.push(u8::from(!negative));
    buf.extend_from_slice(&(if negative { !biased } else { biased }).to_be_bytes());
    if negative {
        buf.extend(digits.into_iter().map(|digit| !digit));
        buf.push(0xFF);
    } else {
        buf.extend_from_slice(&digits);
        buf.push(0x00);
    }
    Ok(())
}

fn decode_decimal(bytes: &[u8]) -> Result<(Value, usize), StorageError> {
    if bytes.len() < 7 {
        return Err(StorageError::deserialize_error("truncated decimal"));
    }
    let sign = bytes[1];
    if sign > 1 {
        return Err(StorageError::deserialize_error("invalid decimal sign"));
    }
    let raw = u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]);
    let wire_exponent = if sign == 0 { !raw } else { raw };
    let exponent = (wire_exponent ^ 0x8000_0000) as i32;
    let terminator = if sign == 0 { 0xFF } else { 0x00 };
    let mut position = 6;
    let mut digits = Vec::new();
    while position < bytes.len() && bytes[position] != terminator {
        let digit = if sign == 0 {
            !bytes[position]
        } else {
            bytes[position]
        };
        if !digit.is_ascii_digit() {
            return Err(StorageError::deserialize_error(
                "invalid decimal significand",
            ));
        }
        digits.push(digit);
        position += 1;
    }
    if digits.is_empty() || position == bytes.len() {
        return Err(StorageError::deserialize_error(
            "missing decimal terminator",
        ));
    }
    position += 1;
    if digits.iter().all(|digit| *digit == b'0') {
        let value = "0"
            .parse::<crate::core::value::Decimal128Value>()
            .map_err(|error| {
                StorageError::deserialize_error(format!("invalid decimal: {error}"))
            })?;
        return Ok((Value::Decimal128(value), position));
    }
    let decimal_position = exponent;
    let digit_count = i32::try_from(digits.len())
        .map_err(|_| StorageError::deserialize_error("decimal significand is too long"))?;
    let exponent10 = decimal_position
        .checked_sub(digit_count)
        .ok_or_else(|| StorageError::deserialize_error("decimal exponent is out of range"))?;
    let mut text = String::new();
    if sign == 0 && digits.iter().any(|digit| *digit != b'0') {
        text.push('-');
    }
    text.extend(digits.iter().map(|digit| char::from(*digit)));
    if exponent10 != 0 {
        text.push('e');
        text.push_str(&exponent10.to_string());
    }
    let value = text
        .parse::<crate::core::value::Decimal128Value>()
        .map_err(|error| StorageError::deserialize_error(format!("invalid decimal: {error}")))?;
    Ok((Value::Decimal128(value), position))
}

impl Default for OrderedCodec {
    fn default() -> Self {
        Self::new()
    }
}

// ── VertexId I/O ────────────────────────────────────────────────────────────

/// Write a VertexId as fixed-size 33 bytes for order-preserving comparison.
/// Format: [data:32] [len:1]. The zero padding keeps byte ordering identical
/// to the raw VertexId bytes, including prefix relationships.
fn write_vertex_id(vid: &VertexId, buf: &mut Vec<u8>) {
    let bytes = vid.as_bytes();
    buf.extend_from_slice(bytes);
    let pad = 32usize.saturating_sub(bytes.len());
    buf.extend(std::iter::repeat_n(0u8, pad));
    buf.push(bytes.len() as u8);
}

/// Read a VertexId from 33 fixed-size bytes.
fn read_vertex_id(bytes: &[u8]) -> Result<(VertexId, usize), StorageError> {
    if bytes.len() < 33 {
        return Err(StorageError::deserialize_error(
            "truncated vertex id (need 33)",
        ));
    }
    let len = bytes[32] as usize;
    if len > 32 {
        return Err(StorageError::deserialize_error("invalid vertex id length"));
    }
    if bytes[len..32].iter().any(|byte| *byte != 0) {
        return Err(StorageError::deserialize_error(
            "non-zero vertex id padding",
        ));
    }
    let data = &bytes[..len];
    Ok((VertexId::from_bytes(data.to_vec()), 33))
}

// ── IEEE 754 total-order encoding ───────────────────────────────────────────

fn encode_f32_ordered(f: f32) -> u32 {
    if f.is_nan() {
        return 0;
    }
    if f == 0.0 {
        return 0x8000_0001;
    }
    let bits = f.to_bits();
    let ordered = if f.is_sign_negative() {
        !bits
    } else {
        bits ^ 0x8000_0000
    };
    ordered + 1
}

fn decode_f32_ordered(bits: u32) -> f32 {
    if bits == 0 {
        return f32::NAN;
    }
    let ordered = bits - 1;
    if ordered & 0x8000_0000 != 0 {
        f32::from_bits(ordered ^ 0x8000_0000)
    } else {
        f32::from_bits(!ordered)
    }
}

fn encode_f64_ordered(f: f64) -> u64 {
    if f.is_nan() {
        return 0;
    }
    if f == 0.0 {
        return 0x8000_0000_0000_0001;
    }
    let bits = f.to_bits();
    let ordered = if f.is_sign_negative() {
        !bits
    } else {
        bits ^ 0x8000_0000_0000_0000
    };
    ordered + 1
}

fn decode_f64_ordered(bits: u64) -> f64 {
    if bits == 0 {
        return f64::NAN;
    }
    let ordered = bits - 1;
    if ordered & 0x8000_0000_0000_0000 != 0 {
        f64::from_bits(ordered ^ 0x8000_0000_0000_0000)
    } else {
        f64::from_bits(!ordered)
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
    fn test_null_variants_roundtrip_in_wire_order() {
        let values = [
            NullType::Null,
            NullType::NaN,
            NullType::BadData,
            NullType::BadType,
            NullType::ErrOverflow,
            NullType::UnknownProp,
            NullType::DivByZero,
            NullType::OutOfRange,
        ];
        for pair in values.windows(2) {
            let left = Value::Null(pair[0].clone());
            let right = Value::Null(pair[1].clone());
            assert_eq!(
                codec()
                    .encode(&left)
                    .unwrap()
                    .cmp(&codec().encode(&right).unwrap()),
                std::cmp::Ordering::Less
            );
        }
        for null_type in values {
            let value = Value::Null(null_type);
            assert_eq!(
                codec().decode(&codec().encode(&value).unwrap()).unwrap(),
                value
            );
        }
    }

    #[test]
    fn test_null_last_preserves_empty_and_null_variants() {
        for value in [Value::Empty, Value::Null(NullType::BadType)] {
            let encoded = codec().encode_null_last(&value).unwrap();
            assert_eq!(codec().decode(&encoded).unwrap(), value);
        }
    }

    #[test]
    fn test_empty() {
        let v = Value::Empty;
        let enc = codec().encode(&v).unwrap();
        let dec = codec().decode(&enc).unwrap();
        assert_eq!(dec, v);
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
    fn test_float_special_values_follow_value_order() {
        let nan = Value::Float(f32::NAN);
        let negative_zero = Value::Float(-0.0);
        let positive_zero = Value::Float(0.0);
        let positive = Value::Float(1.0);

        assert_eq!(
            codec().encode(&negative_zero).unwrap(),
            codec().encode(&positive_zero).unwrap()
        );
        assert_eq!(
            codec()
                .encode(&nan)
                .unwrap()
                .cmp(&codec().encode(&positive).unwrap()),
            nan.cmp(&positive)
        );
        assert_eq!(codec().decode(&codec().encode(&nan).unwrap()).unwrap(), nan);
    }

    #[test]
    fn test_double_special_values_follow_value_order() {
        let nan = Value::Double(f64::NAN);
        let negative_zero = Value::Double(-0.0);
        let positive_zero = Value::Double(0.0);
        let positive = Value::Double(1.0);

        assert_eq!(
            codec().encode(&negative_zero).unwrap(),
            codec().encode(&positive_zero).unwrap()
        );
        assert_eq!(
            codec()
                .encode(&nan)
                .unwrap()
                .cmp(&codec().encode(&positive).unwrap()),
            nan.cmp(&positive)
        );
        assert_eq!(codec().decode(&codec().encode(&nan).unwrap()).unwrap(), nan);
    }

    #[test]
    fn test_string_roundtrip() {
        let cases = [
            Value::string(""),
            Value::string("hello"),
            Value::string("世界"),
            Value::FixedString {
                len: 5,
                data: "fixed".to_string(),
            },
        ];
        for v in &cases {
            let enc = codec().encode(v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, *v);
        }
    }

    #[test]
    fn test_string_order_aa_less_than_b() {
        let aa = codec().encode(&Value::string("aa")).unwrap();
        let b = codec().encode(&Value::string("b")).unwrap();
        assert!(
            aa < b,
            "encoded('aa') < encoded('b') must hold for semantic ordering"
        );
    }

    #[test]
    fn test_string_order() {
        let a = codec().encode(&Value::string("a")).unwrap();
        let b = codec().encode(&Value::string("b")).unwrap();
        assert!(a < b);
    }

    #[test]
    fn test_blob_roundtrip() {
        let cases = vec![
            Value::Blob(vec![]),
            Value::Blob(vec![0x01, 0x02, 0x03]),
            Value::Blob(vec![0xFF, 0xFE]),
        ];
        for v in cases {
            let enc = codec().encode(&v).unwrap();
            let dec = codec().decode(&enc).unwrap();
            assert_eq!(dec, v, "roundtrip failed for {:?}", v);
        }
    }

    #[test]
    fn test_blob_roundtrips_zero_byte() {
        let v = Value::Blob(vec![0x00, 0x01]);
        assert_eq!(codec().decode(&codec().encode(&v).unwrap()).unwrap(), v);
    }

    #[test]
    fn test_string_roundtrips_zero_byte() {
        let v = Value::string("a\x00b");
        assert_eq!(codec().decode(&codec().encode(&v).unwrap()).unwrap(), v);
    }

    #[test]
    fn test_prefix_bounds_string() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::string("a"))
            .unwrap();
        // lower = [TAG_STRING, 0x61, 0x00]
        assert_eq!(lower[0], TAG_STRING);
        assert_eq!(&lower[1..lower.len() - 2], b"a");
        assert_eq!(lower[lower.len() - 1], 0x00);

        // upper = [TAG_STRING, 0x62]
        assert_eq!(upper, vec![TAG_STRING, 0x62]);

        // Verify all "a*" strings fall within bounds
        for suffix in &["", "a", "aa", "az"] {
            let s_val = format!("a{}", suffix);
            let enc = codec().encode(&Value::string(s_val)).unwrap();
            assert!(enc.as_slice() >= lower.as_slice(), "{:?} >= lower", suffix);
            assert!(enc.as_slice() < upper.as_slice(), "{:?} < upper", suffix);
        }

        // "b" is excluded
        let b_enc = codec().encode(&Value::string("b")).unwrap();
        assert!(b_enc.as_slice() >= upper.as_slice());
    }

    #[test]
    fn test_prefix_bounds_multi_byte() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::string("ab"))
            .unwrap();
        assert_eq!(&lower[1..lower.len() - 2], b"ab");
        assert_eq!(upper, vec![TAG_STRING, 0x61, 0x63]);

        let aba = codec().encode(&Value::string("aba")).unwrap();
        assert!(aba.as_slice() < upper.as_slice());

        let ac = codec().encode(&Value::string("ac")).unwrap();
        assert!(ac.as_slice() >= upper.as_slice());
    }

    #[test]
    fn test_prefix_bounds_blob() {
        let (lower, upper) = codec()
            .prefix_bounds(&Value::Blob(vec![0x01, 0x02]))
            .unwrap();
        assert_eq!(lower[0], TAG_BLOB);
        assert_eq!(&lower[1..lower.len() - 2], &[0x01, 0x02]);
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
            (Value::string("aa"), Value::string("b")),
            (Value::string("a"), Value::string("aa")),
            (Value::string(""), Value::string("a")),
            (Value::string("a"), Value::string("ab")),
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
        assert_eq!(&lower[1..lower.len() - 2], &[0x01, 0xFF]);
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
        let v1 = Value::string("name");
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
        let pre = Value::string("a");
        let k1 = codec()
            .encode_composite(&[&pre, &Value::Int(1)], None, false)
            .unwrap();
        let k2 = codec()
            .encode_composite(&[&pre, &Value::Int(2)], None, false)
            .unwrap();
        assert!(k1 < k2);

        let k3 = codec()
            .encode_composite(
                &[&Value::string("b"), &Value::Int(0)],
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
        let v = Value::string("hello");
        let key = codec()
            .encode_composite(&[&v], Some(&EntityRef::Vertex(vid)), false)
            .unwrap();
        let (dec, _c) = codec().decode_value_inner(&key).unwrap();
        assert_eq!(dec, v);
    }

    #[test]
    fn test_vertex_id_entity_roundtrip_supports_max_length() {
        let vid = VertexId::from_string("12345678901234567890123456789012");
        let entity = EntityRef::Vertex(vid);
        let mut encoded = Vec::new();
        codec().encode_entity(&entity, &mut encoded).unwrap();

        let (decoded, consumed) = codec().decode_entity_bytes(&encoded).unwrap();
        assert_eq!(decoded, entity);
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn ordered_codec_integer_property() {
        let groups = [
            vec![
                Value::SmallInt(i16::MIN),
                Value::SmallInt(-1),
                Value::SmallInt(0),
                Value::SmallInt(i16::MAX),
            ],
            vec![
                Value::Int(i32::MIN),
                Value::Int(-1),
                Value::Int(0),
                Value::Int(i32::MAX),
            ],
            vec![
                Value::BigInt(i64::MIN),
                Value::BigInt(-1),
                Value::BigInt(0),
                Value::BigInt(i64::MAX),
            ],
        ];
        for values in groups {
            for pair in values.windows(2) {
                let semantic = pair[0].cmp(&pair[1]);
                let encoded = codec()
                    .encode(&pair[0])
                    .unwrap()
                    .cmp(&codec().encode(&pair[1]).unwrap());
                assert_eq!(
                    encoded, semantic,
                    "integer order changed for {:?} and {:?}",
                    pair[0], pair[1]
                );
            }
        }
    }

    #[test]
    fn ordered_codec_string_and_blob_property() {
        let strings = ["", "a", "aa", "b", "世界", "z\0a"];
        for pair in strings.windows(2) {
            let left = Value::string(pair[0]);
            let right = Value::string(pair[1]);
            assert_eq!(
                codec()
                    .encode(&left)
                    .unwrap()
                    .cmp(&codec().encode(&right).unwrap()),
                left.cmp(&right)
            );
        }

        let blobs = [vec![], vec![0], vec![0, 1], vec![1], vec![1, 255], vec![2]];
        for pair in blobs.windows(2) {
            let left = Value::Blob(pair[0].clone());
            let right = Value::Blob(pair[1].clone());
            assert_eq!(
                codec()
                    .encode(&left)
                    .unwrap()
                    .cmp(&codec().encode(&right).unwrap()),
                left.cmp(&right)
            );
        }
    }

    #[test]
    fn ordered_codec_decimal_property() {
        let values = [
            "-100.5", "-1.25", "-0.01", "0", "0.01", "0.1", "1.2", "1.20", "10", "100.5", "1e3",
        ]
        .into_iter()
        .map(|text| Value::Decimal128(text.parse().expect("decimal fixture should parse")))
        .collect::<Vec<_>>();
        for pair in values.windows(2) {
            assert_eq!(
                codec()
                    .encode(&pair[0])
                    .unwrap()
                    .cmp(&codec().encode(&pair[1]).unwrap()),
                pair[0].cmp(&pair[1])
            );
            assert_eq!(
                codec().decode(&codec().encode(&pair[0]).unwrap()).unwrap(),
                pair[0]
            );
        }
    }

    #[test]
    fn ordered_codec_rejects_unrepresentable_decimal_exponents() {
        let bytes = [TAG_DECIMAL, 1, 0, 0, 0, 0, b'1', 0];
        assert!(codec().decode(&bytes).is_err());
    }

    #[test]
    fn ordered_codec_composite_prefix_property() {
        let first = Value::string("user");
        let second = Value::Int(1);
        let third = Value::Int(2);
        let first_key = codec()
            .encode_composite(&[&first, &second], None, false)
            .unwrap();
        let second_key = codec()
            .encode_composite(&[&first, &third], None, false)
            .unwrap();
        let other_key = codec()
            .encode_composite(&[&Value::string("user2"), &second], None, false)
            .unwrap();
        assert!(first_key < second_key);
        assert!(second_key < other_key);
        let (decoded_first, consumed) = codec().decode_value_inner(&first_key).unwrap();
        let (decoded_second, _) = codec().decode_value_inner(&first_key[consumed..]).unwrap();
        assert_eq!(decoded_first, first);
        assert_eq!(decoded_second, second);
    }

    #[test]
    fn ordered_codec_supported_type_tags_follow_value_priority() {
        let values = [
            Value::Empty,
            Value::Null(NullType::Null),
            Value::Bool(false),
            Value::SmallInt(0),
            Value::Int(0),
            Value::BigInt(0),
            Value::Float(0.0),
            Value::Double(0.0),
            Value::Decimal128("0".parse().expect("decimal fixture should parse")),
            Value::Date(DateValue {
                year: 2024,
                month: 1,
                day: 1,
            }),
            Value::Time(TimeValue {
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            Value::DateTime(DateTimeValue {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            Value::string(""),
            Value::FixedString {
                len: 0,
                data: "".to_string(),
            },
            Value::Blob(Vec::new()),
            Value::Uuid(UuidValue::from_bytes([0; 16])),
        ];
        for pair in values.windows(2) {
            assert_eq!(
                codec()
                    .encode(&pair[0])
                    .unwrap()
                    .cmp(&codec().encode(&pair[1]).unwrap()),
                std::cmp::Ordering::Less,
                "type order changed for {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}

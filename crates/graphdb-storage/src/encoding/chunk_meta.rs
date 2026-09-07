//! Per-chunk compression metadata and selection profiles.
//!
//! `ChunkEncodingMeta` records which encoding a chunk uses plus the min/max
//! and sizing inputs needed by the in-place update check. `ChunkProfile` is
//! a single-pass distribution summary used to pick an encoding per chunk.

use std::io::{Read, Write};

use graphdb_core::{DataType, StorageResult, Value};

use super::EncodingType;

/// Per-chunk compression metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkEncodingMeta {
    pub scheme: EncodingType,
    pub num_values: u32,
    pub all_null: bool,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub bit_width: Option<u8>,
    pub alp_exceptions: Option<(u32, u32)>,
    pub compressed_size: u64,
    pub raw_size: u64,
}

impl Default for ChunkEncodingMeta {
    fn default() -> Self {
        Self {
            scheme: EncodingType::None,
            num_values: 0,
            all_null: true,
            min: None,
            max: None,
            bit_width: None,
            alp_exceptions: None,
            compressed_size: 0,
            raw_size: 0,
        }
    }
}

impl ChunkEncodingMeta {
    pub fn memory_usage(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.min.as_ref().map(|v| v.estimated_size()).unwrap_or(0)
            + self.max.as_ref().map(|v| v.estimated_size()).unwrap_or(0)
    }

    pub fn serialize(&self, writer: &mut impl Write) -> StorageResult<usize> {
        let mut written = 0usize;
        writer.write_all(&[self.scheme.to_u8()])?;
        written += 1;
        writer.write_all(&self.num_values.to_le_bytes())?;
        written += 4;
        writer.write_all(&[self.all_null as u8])?;
        written += 1;
        writer.write_all(&[self.min.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.min {
            written += crate::column_stats::serialize_stat_value(writer, v)?;
        }
        writer.write_all(&[self.max.is_some() as u8])?;
        written += 1;
        if let Some(ref v) = self.max {
            written += crate::column_stats::serialize_stat_value(writer, v)?;
        }
        writer.write_all(&[self.bit_width.is_some() as u8])?;
        written += 1;
        if let Some(bw) = self.bit_width {
            writer.write_all(&[bw])?;
            written += 1;
        }
        let (exc, exc_cap) = self.alp_exceptions.unwrap_or((0, 0));
        writer.write_all(&[self.alp_exceptions.is_some() as u8])?;
        written += 1;
        if self.alp_exceptions.is_some() {
            writer.write_all(&exc.to_le_bytes())?;
            writer.write_all(&exc_cap.to_le_bytes())?;
            written += 8;
        }
        writer.write_all(&self.compressed_size.to_le_bytes())?;
        writer.write_all(&self.raw_size.to_le_bytes())?;
        written += 16;
        Ok(written)
    }

    pub fn deserialize(reader: &mut impl Read) -> StorageResult<Self> {
        let mut tag = [0u8; 1];
        reader.read_exact(&mut tag)?;
        let scheme = EncodingType::from_u8(tag[0]);
        let mut u32buf = [0u8; 4];
        reader.read_exact(&mut u32buf)?;
        let num_values = u32::from_le_bytes(u32buf);
        reader.read_exact(&mut tag)?;
        let all_null = tag[0] != 0;
        reader.read_exact(&mut tag)?;
        let has_min = tag[0] != 0;
        let min = if has_min {
            Some(crate::column_stats::deserialize_stat_value(reader)?)
        } else {
            None
        };
        reader.read_exact(&mut tag)?;
        let has_max = tag[0] != 0;
        let max = if has_max {
            Some(crate::column_stats::deserialize_stat_value(reader)?)
        } else {
            None
        };
        let mut tag = [0u8; 1];
        reader.read_exact(&mut tag)?;
        let has_bw = tag[0] != 0;
        let bw_val = if has_bw {
            let mut bw = [0u8; 1];
            reader.read_exact(&mut bw)?;
            Some(bw[0])
        } else {
            None
        };
        let mut u32b = [0u8; 4];
        reader.read_exact(&mut tag)?;
        let has_exc = tag[0] != 0;
        let alp_exceptions = if has_exc {
            reader.read_exact(&mut u32b)?;
            let exc_count = u32::from_le_bytes(u32b);
            reader.read_exact(&mut u32b)?;
            let exc_cap = u32::from_le_bytes(u32b);
            Some((exc_count, exc_cap))
        } else {
            None
        };
        let mut u64b = [0u8; 8];
        reader.read_exact(&mut u64b)?;
        let compressed_size = u64::from_le_bytes(u64b);
        reader.read_exact(&mut u64b)?;
        let raw_size = u64::from_le_bytes(u64b);
        Ok(Self {
            scheme,
            num_values,
            all_null,
            min,
            max,
            bit_width: bw_val,
            alp_exceptions,
            compressed_size,
            raw_size,
        })
    }
}

/// Single-pass distribution summary of one chunk used for encoding selection.
#[derive(Debug, Clone)]
pub struct ChunkProfile {
    pub data_type: DataType,
    pub num_values: usize,
    pub null_count: usize,
    pub min: Option<Value>,
    pub max: Option<Value>,
    pub bit_width: Option<u8>,
    pub run_ratio: Option<f64>,
    pub distinct: Option<usize>,
    pub total_str_len: Option<usize>,
    pub hot_update: bool,
}

impl Default for ChunkProfile {
    fn default() -> Self {
        Self {
            data_type: DataType::Empty,
            num_values: 0,
            null_count: 0,
            min: None,
            max: None,
            bit_width: None,
            run_ratio: None,
            distinct: None,
            total_str_len: None,
            hot_update: false,
        }
    }
}

/// Scan one chunk worth of values into a profile without retaining them.
pub fn profile_chunk(
    values: impl Iterator<Item = Option<Value>>,
    data_type: &DataType,
    hot_update: bool,
) -> ChunkProfile {
    let mut profile = ChunkProfile {
        data_type: data_type.clone(),
        hot_update,
        ..Default::default()
    };
    let mut prev_int: Option<i64> = None;
    let mut runs: usize = 0;
    let mut total: usize = 0;
    let mut distinct_int: Option<std::collections::HashSet<i64>> = None;
    let mut distinct_str: Option<std::collections::HashSet<String>> = None;
    let is_int = matches!(
        data_type,
        DataType::SmallInt | DataType::Int | DataType::BigInt
    );
    let is_str = matches!(data_type, DataType::String);
    if is_str {
        distinct_str = Some(std::collections::HashSet::new());
    }
    if is_int {
        distinct_int = Some(std::collections::HashSet::new());
    }
    let mut min_i64: Option<i64> = None;
    let mut max_i64: Option<i64> = None;
    let mut str_len: usize = 0;

    for v in values {
        match v {
            None => profile.null_count += 1,
            Some(val) => {
                profile.num_values += 1;
                total += 1;
                if profile.min.is_none()
                    || crate::vertex::column::compare_values(
                        &val,
                        profile.min.as_ref().unwrap_or(&val),
                    ) == std::cmp::Ordering::Less
                {
                    profile.min = Some(val.clone());
                }
                if profile.max.is_none()
                    || crate::vertex::column::compare_values(
                        profile.max.as_ref().unwrap_or(&val),
                        &val,
                    ) == std::cmp::Ordering::Less
                {
                    profile.max = Some(val.clone());
                }
                match &val {
                    Value::SmallInt(i) => {
                        let iv = *i as i64;
                        min_i64 = Some(min_i64.map_or(iv, |m: i64| m.min(iv)));
                        max_i64 = Some(max_i64.map_or(iv, |m: i64| m.max(iv)));
                        if let Some(s) = distinct_int.as_mut() {
                            s.insert(iv);
                        }
                        if let Some(p) = prev_int {
                            if p != iv {
                                runs += 1;
                            }
                        } else if total > 1 {
                            runs += 1;
                        }
                        prev_int = Some(iv);
                    }
                    Value::Int(i) => {
                        let iv = *i as i64;
                        min_i64 = Some(min_i64.map_or(iv, |m: i64| m.min(iv)));
                        max_i64 = Some(max_i64.map_or(iv, |m: i64| m.max(iv)));
                        if let Some(s) = distinct_int.as_mut() {
                            s.insert(iv);
                        }
                        if let Some(p) = prev_int {
                            if p != iv {
                                runs += 1;
                            }
                        } else if total > 1 {
                            runs += 1;
                        }
                        prev_int = Some(iv);
                    }
                    Value::BigInt(iv) => {
                        min_i64 = Some(min_i64.map_or(*iv, |m: i64| m.min(*iv)));
                        max_i64 = Some(max_i64.map_or(*iv, |m: i64| m.max(*iv)));
                        if let Some(s) = distinct_int.as_mut() {
                            s.insert(*iv);
                        }
                        if let Some(p) = prev_int {
                            if p != *iv {
                                runs += 1;
                            }
                        } else if total > 1 {
                            runs += 1;
                        }
                        prev_int = Some(*iv);
                    }
                    Value::String(s) => {
                        str_len += s.len();
                        if let Some(set) = distinct_str.as_mut() {
                            set.insert(s.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if is_int {
        if let (Some(lo), Some(hi)) = (min_i64, max_i64) {
            let range = hi.saturating_sub(lo) as u64;
            let bw = if range == 0 {
                1
            } else {
                (64 - range.leading_zeros()) as u8
            };
            profile.bit_width = Some(bw);
        }
        if total > 0 {
            profile.run_ratio = Some((runs + 1) as f64 / total as f64);
        }
        if let Some(s) = distinct_int {
            profile.distinct = Some(s.len());
        }
    }
    if is_str {
        if let Some(s) = distinct_str {
            profile.distinct = Some(s.len());
        }
        profile.total_str_len = Some(str_len);
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_roundtrip() {
        let meta = ChunkEncodingMeta {
            scheme: EncodingType::BitPacking,
            num_values: 10,
            all_null: false,
            min: Some(Value::Int(1)),
            max: Some(Value::Int(9)),
            bit_width: Some(4),
            alp_exceptions: None,
            compressed_size: 100,
            raw_size: 200,
        };
        let mut buf = Vec::new();
        meta.serialize(&mut buf).unwrap();
        let back = ChunkEncodingMeta::deserialize(&mut &buf[..]).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn profile_int_runs() {
        let values: Vec<Option<Value>> = (0..10).map(|_| Some(Value::Int(5))).collect();
        let p = profile_chunk(values.into_iter(), &DataType::Int, false);
        assert_eq!(p.num_values, 10);
        assert_eq!(p.bit_width, Some(1));
        assert!(p.run_ratio.unwrap() < 0.2);
    }
}

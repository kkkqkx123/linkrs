//! Storage statistics helpers: HyperLogLog cardinality estimator.
//!
//! Precision target is P=6 (M=64 registers): a small 64-byte footprint per
//! column with an expected relative error around 13%. Callers needing
//! tighter bounds should document the measured error next to the estimate.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};

use graphdb_core::{StorageResult, Value};

pub const HLL_P: u32 = 6;
pub const HLL_M: usize = 64;
const HLL_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperLogLog {
    registers: [u8; HLL_M],
}

impl Default for HyperLogLog {
    fn default() -> Self {
        Self::new()
    }
}

impl HyperLogLog {
    pub fn new() -> Self {
        Self {
            registers: [0; HLL_M],
        }
    }

    pub fn add_value(&mut self, value: &Value) {
        match value {
            Value::Double(f) if f.is_nan() => (),
            Value::Float(f) if f.is_nan() => (),
            Value::Null(_) | Value::Empty => (),
            Value::Double(f) => self.add_hash(f.to_bits()),
            Value::Float(f) => self.add_hash(f.to_bits() as u64),
            _ => {
                let mut hasher = DefaultHasher::new();
                hash_value(value, &mut hasher);
                self.add_hash(hasher.finish());
            }
        }
    }

    pub fn add_hash(&mut self, hash: u64) {
        let idx = (hash & (HLL_M as u64 - 1)) as usize;
        let w = hash >> HLL_P;
        let rank = (w.leading_zeros() - HLL_P + 1).min(64 - HLL_P) as u8;
        if rank > self.registers[idx] {
            self.registers[idx] = rank;
        }
    }

    pub fn estimate(&self) -> u64 {
        let m = HLL_M as f64;
        let alpha = 0.673;
        let mut sum = 0.0;
        let mut zeros = 0u32;
        for r in &self.registers {
            sum += 2.0f64.powi(-(*r as i32));
            if *r == 0 {
                zeros += 1;
            }
        }
        let raw = alpha * m * m / sum;
        let estimate = if raw <= 2.5 * m && zeros > 0 {
            m * (m / zeros as f64).ln()
        } else {
            raw
        };
        estimate.round() as u64
    }

    pub fn merge(&mut self, other: &Self) {
        for (a, b) in self.registers.iter_mut().zip(other.registers.iter()) {
            if *b > *a {
                *a = *b;
            }
        }
    }

    pub fn registers(&self) -> &[u8; HLL_M] {
        &self.registers
    }

    pub fn serialize(&self, writer: &mut impl Write) -> StorageResult<usize> {
        writer.write_all(&[HLL_VERSION])?;
        writer.write_all(&self.registers)?;
        Ok(1 + HLL_M)
    }

    pub fn deserialize(reader: &mut impl Read) -> StorageResult<Self> {
        let mut ver = [0u8; 1];
        reader.read_exact(&mut ver)?;
        if ver[0] != HLL_VERSION {
            return Err(graphdb_core::StorageError::deserialize_error(format!(
                "unsupported HLL version {}",
                ver[0]
            )));
        }
        let mut registers = [0u8; HLL_M];
        reader.read_exact(&mut registers)?;
        Ok(Self { registers })
    }
}

fn hash_value(value: &Value, hasher: &mut DefaultHasher) {
    match value {
        Value::Bool(b) => b.hash(hasher),
        Value::SmallInt(v) => v.hash(hasher),
        Value::Int(v) => v.hash(hasher),
        Value::BigInt(v) => v.hash(hasher),
        Value::String(s) => s.as_str().hash(hasher),
        Value::FixedString(s) => s.hash(hasher),
        Value::Blob(b) => b.hash(hasher),
        Value::Date(d) => d.hash(hasher),
        Value::Time(t) => t.hash(hasher),
        Value::DateTime(dt) => dt.hash(hasher),
        Value::Uuid(u) => u.hash(hasher),
        Value::Json(j) => j.as_str().hash(hasher),
        Value::JsonB(j) => j.to_json_string().hash(hasher),
        _ => {
            std::mem::discriminant(value).hash(hasher);
            value.estimated_size().hash(hasher);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_estimate_is_zero() {
        assert_eq!(HyperLogLog::new().estimate(), 0);
    }

    #[test]
    fn single_value_estimate_near_one() {
        let mut hll = HyperLogLog::new();
        hll.add_value(&Value::Int(42));
        let est = hll.estimate();
        assert!((1..=3).contains(&est), "estimate={}", est);
    }

    #[test]
    fn merge_union_matches_full() {
        let mut a = HyperLogLog::new();
        let mut b = HyperLogLog::new();
        let mut full = HyperLogLog::new();
        for i in 0..1000i32 {
            let v = Value::Int(i);
            full.add_value(&v);
            if i % 2 == 0 {
                a.add_value(&v);
            } else {
                b.add_value(&v);
            }
        }
        a.merge(&b);
        let merged = a.estimate() as f64;
        let expected = full.estimate() as f64;
        let err = (merged - expected).abs() / expected.max(1.0);
        assert!(err < 0.15, "merged={} expected={}", merged, expected);
    }

    #[test]
    fn roundtrip() {
        let mut hll = HyperLogLog::new();
        hll.add_value(&Value::string("hello"));
        let mut buf = Vec::new();
        hll.serialize(&mut buf).unwrap();
        assert_eq!(buf.len(), 65);
        let back = HyperLogLog::deserialize(&mut &buf[..]).unwrap();
        assert_eq!(back, hll);
    }

    #[test]
    fn nan_skipped() {
        let mut hll = HyperLogLog::new();
        hll.add_value(&Value::Double(f64::NAN));
        assert_eq!(hll.estimate(), 0);
    }
}

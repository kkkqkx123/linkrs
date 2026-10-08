//! Storage statistics helpers: HyperLogLog cardinality estimator.
//!
//! Precision target is P=6 (M=64 registers): a small 64-byte footprint per
//! column with an expected relative error around 13%. Callers needing
//! tighter bounds should document the measured error next to the estimate.
//!
//! Hashing is deterministic FNV-1a so persisted registers stay comparable
//! across processes and restarts.

use std::io::{Read, Write};

use linkrs_core::{StorageResult, Value};

pub const HLL_P: u32 = 6;
pub const HLL_M: usize = 64;

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
                self.add_hash(fnv1a_value(value));
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
        writer.write_all(&self.registers)?;
        Ok(HLL_M)
    }

    pub fn deserialize(reader: &mut impl Read) -> StorageResult<Self> {
        let mut registers = [0u8; HLL_M];
        reader.read_exact(&mut registers)?;
        Ok(Self { registers })
    }
}

/// Deterministic 64-bit FNV-1a over a stable per-type encoding.
///
/// Replaces the process-seeded default hasher so HyperLogLog registers can be
/// persisted and merged across restarts without changing the estimate.
fn fnv1a_value(value: &Value) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(PRIME);
        }
    };
    // Discriminant first so different types never collide on equal payloads.
    mix(&[value_discriminant(value)]);
    match value {
        Value::Bool(b) => mix(&[*b as u8]),
        Value::SmallInt(v) => mix(&v.to_le_bytes()),
        Value::Int(v) => mix(&v.to_le_bytes()),
        Value::BigInt(v) => mix(&v.to_le_bytes()),
        Value::String(s) => mix(s.as_bytes()),
        Value::FixedString(s) => mix(s.as_bytes()),
        Value::Blob(b) => mix(b),
        Value::Date(d) => {
            mix(&d.year.to_le_bytes());
            mix(&d.month.to_le_bytes());
            mix(&d.day.to_le_bytes());
        }
        Value::Time(t) => {
            mix(&t.hour.to_le_bytes());
            mix(&t.minute.to_le_bytes());
            mix(&t.sec.to_le_bytes());
            mix(&t.microsec.to_le_bytes());
        }
        Value::DateTime(dt) => {
            mix(&dt.year.to_le_bytes());
            mix(&dt.month.to_le_bytes());
            mix(&dt.day.to_le_bytes());
            mix(&dt.hour.to_le_bytes());
            mix(&dt.minute.to_le_bytes());
            mix(&dt.sec.to_le_bytes());
            mix(&dt.microsec.to_le_bytes());
        }
        Value::Uuid(u) => mix(u.as_bytes()),
        Value::Json(j) => mix(j.as_str().as_bytes()),
        Value::JsonB(j) => mix(j.to_json_string().as_bytes()),
        _ => {
            if let Ok(bytes) = postcard::to_allocvec(value) {
                mix(&bytes);
            } else {
                mix(&value.estimated_size().to_le_bytes());
            }
        }
    }
    // FNV-1a leaves sequential inputs correlated in the low bits that back
    // the register index, so run a splitmix64 finalizer for avalanche while
    // staying dependency-free and deterministic.
    mix64(hash)
}

/// SplitMix64 finalizer: strong bit avalanche for a 64-bit hash.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    x
}

/// Stable per-type discriminant for the FNV-1a encoding.
fn value_discriminant(value: &Value) -> u8 {
    use linkrs_core::value::Value::*;
    match value {
        Null(_) => 0,
        Empty => 1,
        Bool(_) => 2,
        SmallInt(_) => 3,
        Int(_) => 4,
        BigInt(_) => 5,
        Float(_) => 6,
        Double(_) => 7,
        String(_) => 8,
        FixedString(_) => 9,
        Blob(_) => 10,
        Date(_) => 11,
        Time(_) => 12,
        DateTime(_) => 13,
        Uuid(_) => 14,
        Json(_) => 15,
        JsonB(_) => 16,
        _ => 255,
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
        assert_eq!(buf.len(), 64);
        let back = HyperLogLog::deserialize(&mut &buf[..]).unwrap();
        assert_eq!(back, hll);
    }

    #[test]
    fn nan_skipped() {
        let mut hll = HyperLogLog::new();
        hll.add_value(&Value::Double(f64::NAN));
        assert_eq!(hll.estimate(), 0);
    }

    #[test]
    fn deterministic_registers_across_instances() {
        let values: Vec<Value> = (0..200i32).map(Value::Int).collect();
        let mut a = HyperLogLog::new();
        let mut b = HyperLogLog::new();
        for v in &values {
            a.add_value(v);
        }
        for v in values.iter().rev() {
            b.add_value(v);
        }
        assert_eq!(a.registers(), b.registers());
        assert_eq!(a.estimate(), b.estimate());
    }
}

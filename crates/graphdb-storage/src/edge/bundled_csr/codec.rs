use graphdb_core::Value;

/// Encode a scalar `Value` into a 64-bit storage word.
///
/// `Bool` uses 0/1, `Float` transmutes via `f32::to_bits`, and all integer
/// and time types are widened to `i64`.  Types not representable in 64 bits
/// are rejected at table creation; this function assumes the caller already
/// validated the type.
pub fn encode_scalar(v: &Value) -> u64 {
    match v {
        Value::Bool(b) => u64::from(*b),
        Value::SmallInt(n) => *n as u64,
        Value::Int(n) => *n as u64,
        Value::BigInt(n) => *n as u64,
        Value::Float(f) => f.to_bits() as u64,
        Value::Double(f) => f.to_bits(),
        Value::Date(d) => d.to_days() as u64,
        Value::Time(t) => {
            (t.hour as i64 * 3_600_000_000
                + t.minute as i64 * 60_000_000
                + t.sec as i64 * 1_000_000
                + t.microsec as i64) as u64
        }
        Value::DateTime(dt) => dt.to_micros() as u64,
        _ => 0,
    }
}

/// Decode a 64-bit storage word back into a `Value` of the given `DataType`.
pub fn decode_scalar(raw: u64, dt: &graphdb_core::DataType) -> Value {
    use graphdb_core::DataType;
    match dt {
        DataType::Bool => Value::Bool(raw != 0),
        DataType::SmallInt => Value::SmallInt(raw as i16),
        DataType::Int => Value::Int(raw as i32),
        DataType::BigInt => Value::BigInt(raw as i64),
        DataType::Float => Value::Float(f32::from_bits(raw as u32)),
        DataType::Double => Value::Double(f64::from_bits(raw)),
        DataType::Date => Value::Date(graphdb_core::value::DateValue::from_days(raw as i64)),
        DataType::Time => {
            let total_micros = raw as i64;
            let microsec = (total_micros % 1_000_000) as u32;
            let total_secs = total_micros / 1_000_000;
            let sec = (total_secs % 60) as u32;
            let total_mins = total_secs / 60;
            let minute = (total_mins % 60) as u32;
            let hour = (total_mins / 60) as u32;
            Value::Time(graphdb_core::value::TimeValue {
                hour,
                minute,
                sec,
                microsec,
            })
        }
        DataType::DateTime => {
            Value::DateTime(graphdb_core::value::DateTimeValue::from_micros(raw as i64))
        }
        _ => Value::Empty,
    }
}

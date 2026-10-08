//! Shared wire value conversion.
//!
//! Covers the primitive `Value` mapping used by query, batch and vector
//! handlers plus wall-clock helpers. Domain-specific mappings stay with
//! their handler modules.

use std::time::{SystemTime, UNIX_EPOCH};

/// Convert a proto `Value` to a core `Value`.
///
/// The wire value vocabulary is intentionally narrow; complex core types
/// have no proto spelling and never appear here. Timestamps cross the wire
/// as epoch millis and stay `BigInt` so no timezone interpretation is
/// smuggled in.
pub(crate) fn proto_value_to_core(value: super::proto::Value) -> linkrs_core::Value {
    use super::proto::value::Value as ProtoValue;
    match value.value {
        None => linkrs_core::Value::Empty,
        Some(ProtoValue::StringValue(s)) => linkrs_core::Value::string(s),
        Some(ProtoValue::IntValue(i)) => {
            if i >= i64::from(i32::MIN) && i <= i64::from(i32::MAX) {
                linkrs_core::Value::Int(i as i32)
            } else {
                linkrs_core::Value::BigInt(i)
            }
        }
        Some(ProtoValue::DoubleValue(d)) => linkrs_core::Value::Double(d),
        Some(ProtoValue::FloatValue(f)) => linkrs_core::Value::Float(f as f32),
        Some(ProtoValue::BoolValue(b)) => linkrs_core::Value::Bool(b),
        Some(ProtoValue::BytesValue(b)) => linkrs_core::Value::Blob(b),
        Some(ProtoValue::TimestampValue(t)) => linkrs_core::Value::BigInt(t),
    }
}

/// Convert a core [`Value`] to a protobuf [`super::proto::Value`].
pub(crate) fn value_to_proto_value(value: linkrs_core::Value) -> super::proto::Value {
    use super::proto::value::Value as ProtoValue;
    use super::proto::Value as ProtoValueMsg;

    let proto_val = match value {
        linkrs_core::Value::Empty | linkrs_core::Value::Null(_) => {
            ProtoValue::StringValue(String::new())
        }
        linkrs_core::Value::Bool(b) => ProtoValue::BoolValue(b),
        linkrs_core::Value::SmallInt(i) => ProtoValue::IntValue(i as i64),
        linkrs_core::Value::Int(i) => ProtoValue::IntValue(i as i64),
        linkrs_core::Value::BigInt(i) => ProtoValue::IntValue(i),
        linkrs_core::Value::Float(f) => ProtoValue::FloatValue(f as f64),
        linkrs_core::Value::Double(d) => ProtoValue::DoubleValue(d),
        linkrs_core::Value::Decimal128(d) => ProtoValue::StringValue(d.to_string()),
        linkrs_core::Value::String(s) => ProtoValue::StringValue(s.to_string()),
        linkrs_core::Value::FixedString(data) => ProtoValue::StringValue(data),
        linkrs_core::Value::Date(d) => ProtoValue::StringValue(d.to_string()),
        linkrs_core::Value::Time(t) => ProtoValue::StringValue(t.to_string()),
        linkrs_core::Value::DateTime(dt) => ProtoValue::StringValue(dt.to_string()),
        linkrs_core::Value::Blob(b) => ProtoValue::BytesValue(b),
        other => ProtoValue::StringValue(format!("{:?}", other)),
    };

    ProtoValueMsg {
        value: Some(proto_val),
    }
}

/// Seconds since the Unix epoch for a `SystemTime`, saturating at zero.
pub(crate) fn system_time_secs(time: &SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Wall-clock start of a query profile in epoch millis.
///
/// Profiles only record a monotonic `Instant`; wall time is derived as
/// now minus elapsed, which is exact up to clock adjustments.
pub(crate) fn profile_start_ms(profile: &linkrs_metrics::QueryProfile) -> i64 {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now_ms.saturating_sub(profile.start_time.elapsed().as_millis() as i64)
}

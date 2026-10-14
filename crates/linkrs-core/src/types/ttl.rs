use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::property::PropertyDef;
use crate::value::Value;
use crate::DataType;

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn is_ttl_column_type(data_type: &DataType) -> bool {
    matches!(
        data_type,
        DataType::SmallInt | DataType::Int | DataType::BigInt | DataType::DateTime
    )
}

pub fn validate_ttl(
    properties: &[PropertyDef],
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
) -> Result<(), String> {
    match (ttl_duration, ttl_col) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(
            "TTL requires both ttl_duration and ttl_col to be specified together; omit both to disable TTL"
                .to_string(),
        ),
        (Some(duration), Some(col)) => {
            if duration <= 0 {
                return Err(format!(
                    "ttl_duration must be a positive integer (seconds), got {duration}; omit both ttl_duration and ttl_col to disable TTL"
                ));
            }
            if col.is_empty() {
                return Err(
                    "ttl_col must not be empty; omit both ttl_duration and ttl_col to disable TTL"
                        .to_string(),
                );
            }
            let prop = properties.iter().find(|p| p.name == col).ok_or_else(|| {
                format!("ttl_col '{col}' does not match any property of the schema")
            })?;
            if !is_ttl_column_type(&prop.data_type) {
                return Err(format!(
                    "ttl_col '{col}' has type {}, only integer types and TIMESTAMP/DATETIME can be used as ttl_col",
                    prop.data_type
                ));
            }
            Ok(())
        }
    }
}

pub fn ttl_value_to_epoch_secs(value: &Value) -> Option<i64> {
    match value {
        Value::SmallInt(v) => Some(*v as i64),
        Value::Int(v) => Some(*v as i64),
        Value::BigInt(v) => Some(*v),
        Value::DateTime(dt) => Some(dt.to_micros().div_euclid(1_000_000)),
        Value::Null(_) | Value::Empty => None,
        _ => None,
    }
}

pub fn is_expired(duration_secs: i64, col_value: Option<&Value>, now_secs: i64) -> bool {
    if duration_secs <= 0 {
        return false;
    }
    let Some(value) = col_value else {
        return false;
    };
    let Some(base) = ttl_value_to_epoch_secs(value) else {
        return false;
    };
    now_secs > base.saturating_add(duration_secs)
}

pub fn is_record_expired(
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
    props: &HashMap<Arc<str>, Value>,
) -> bool {
    let (Some(duration), Some(col)) = (ttl_duration, ttl_col) else {
        return false;
    };
    if duration <= 0 || col.is_empty() {
        return false;
    }
    is_expired(duration, props.get(col), now_secs())
}

pub fn is_record_expired_at(
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
    props: &HashMap<Arc<str>, Value>,
    now: i64,
) -> bool {
    let (Some(duration), Some(col)) = (ttl_duration, ttl_col) else {
        return false;
    };
    if duration <= 0 || col.is_empty() {
        return false;
    }
    is_expired(duration, props.get(col), now)
}

pub fn has_valid_ttl(ttl_duration: Option<i64>, ttl_col: Option<&str>) -> bool {
    matches!((ttl_duration, ttl_col), (Some(d), Some(c)) if d > 0 && !c.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::date_time::DateTimeValue;

    fn int_prop(name: &str) -> PropertyDef {
        PropertyDef::new(name.to_string(), DataType::BigInt)
    }

    fn datetime_prop(name: &str) -> PropertyDef {
        PropertyDef::new(name.to_string(), DataType::DateTime)
    }

    fn string_prop(name: &str) -> PropertyDef {
        PropertyDef::new(name.to_string(), DataType::String)
    }

    #[test]
    fn disabled_when_both_absent() {
        assert!(validate_ttl(&[], None, None).is_ok());
        assert!(!has_valid_ttl(None, None));
    }

    #[test]
    fn one_sided_ttl_is_rejected() {
        assert!(validate_ttl(&[], Some(100), None).is_err());
        assert!(validate_ttl(&[], None, Some("c")).is_err());
    }

    #[test]
    fn non_positive_duration_is_rejected() {
        let props = vec![int_prop("created_at")];
        assert!(validate_ttl(&props, Some(0), Some("created_at")).is_err());
        assert!(validate_ttl(&props, Some(-5), Some("created_at")).is_err());
    }

    #[test]
    fn missing_column_is_rejected() {
        let props = vec![int_prop("created_at")];
        assert!(validate_ttl(&props, Some(100), Some("missing")).is_err());
        assert!(validate_ttl(&props, Some(100), Some("")).is_err());
    }

    #[test]
    fn wrong_column_type_is_rejected() {
        let props = vec![string_prop("name")];
        assert!(validate_ttl(&props, Some(100), Some("name")).is_err());
        let props = vec![datetime_prop("ts")];
        assert!(validate_ttl(&props, Some(100), Some("ts")).is_ok());
        let props = vec![int_prop("n")];
        assert!(validate_ttl(&props, Some(100), Some("n")).is_ok());
    }

    #[test]
    fn expiry_boundary_matches_nebula_semantics() {
        assert!(is_expired(100, Some(&Value::BigInt(1000)), 1101));
        assert!(!is_expired(100, Some(&Value::BigInt(1000)), 1100));
        assert!(!is_expired(100, None, 9999));
        assert!(!is_expired(
            100,
            Some(&Value::Null(crate::NullType::Null)),
            9999
        ));
        assert!(!is_expired(100, Some(&Value::string("x")), 9999));
        assert!(!is_expired(0, Some(&Value::BigInt(0)), i64::MAX));
    }

    #[test]
    fn datetime_values_expire_like_integers() {
        let dt = DateTimeValue::from_micros(1_000_000_000 * 1_000_000);
        assert!(is_expired(
            100,
            Some(&Value::DateTime(dt)),
            1_000_000_000 + 101
        ));
        let dt = DateTimeValue::from_micros(1_000_000_000 * 1_000_000);
        assert!(!is_expired(
            100,
            Some(&Value::DateTime(dt)),
            1_000_000_000 + 100
        ));
    }

    #[test]
    fn record_helper_respects_disabled_ttl() {
        let props: HashMap<Arc<str>, Value> = [(Arc::<str>::from("created_at"), Value::BigInt(0))]
            .into_iter()
            .collect();
        assert!(!is_record_expired_at(None, None, &props, i64::MAX));
        assert!(!is_record_expired_at(
            Some(0),
            Some("created_at"),
            &props,
            i64::MAX
        ));
        assert!(is_record_expired_at(
            Some(10),
            Some("created_at"),
            &props,
            11
        ));
    }
}

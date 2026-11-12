use linkrs_core::value::NullType;
use linkrs_core::Value;

pub(crate) fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::BigInt(n) => Some(*n as f64),
        Value::Float(f) => Some(*f as f64),
        Value::Double(f) => Some(*f),
        _ => None,
    }
}

pub(crate) fn integral_or_double(value: f64) -> Value {
    if value.fract() == 0.0 && value.is_finite() {
        Value::BigInt(value as i64)
    } else {
        Value::Double(value)
    }
}

pub(crate) fn welford_update(n: &mut u64, mean: &mut f64, m2: &mut f64, x: f64) {
    *n += 1;
    let delta = x - *mean;
    *mean += delta / *n as f64;
    let delta2 = x - *mean;
    *m2 += delta * delta2;
}

pub(crate) fn welford_merge(n1: &mut u64, mean1: &mut f64, m21: &mut f64, n2: &u64, mean2: &f64, m22: &f64) {
    if *n2 == 0 {
        return;
    }
    if *n1 == 0 {
        *n1 = *n2;
        *mean1 = *mean2;
        *m21 = *m22;
        return;
    }
    let total = *n1 + *n2;
    let delta = mean2 - *mean1;
    let new_mean = (*n1 as f64 * *mean1 + *n2 as f64 * *mean2) / total as f64;
    let new_m2 = *m21 + *m22 + delta * delta * *n1 as f64 * *n2 as f64 / total as f64;
    *n1 = total;
    *mean1 = new_mean;
    *m21 = new_m2;
}

pub(crate) fn percentile_of(values: &[f64], percentile: f64) -> Value {
    if values.is_empty() || !(0.0..=100.0).contains(&percentile) {
        return Value::Null(NullType::Null);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let index = (percentile / 100.0) * (sorted.len() - 1) as f64;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    if lower == upper {
        Value::Double(sorted[lower])
    } else {
        let weight = index - lower as f64;
        Value::Double(sorted[lower] + weight * (sorted[upper] - sorted[lower]))
    }
}

pub(crate) fn median_of(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null(NullType::Null);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let len = sorted.len();
    if len.is_multiple_of(2) {
        Value::Double((sorted[len / 2 - 1] + sorted[len / 2]) / 2.0)
    } else {
        Value::Double(sorted[len / 2])
    }
}

pub(crate) fn mode_of(values: &[Value]) -> Value {
    if values.is_empty() {
        return Value::Null(NullType::Null);
    }
    let mut frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for value in values {
        *frequency.entry(format!("{}", value)).or_insert(0) += 1;
    }
    let Some(mode_str) = frequency
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(key, _)| key)
    else {
        return Value::Null(NullType::Null);
    };
    if let Ok(int_val) = mode_str.parse::<i32>() {
        Value::Int(int_val)
    } else if let Ok(float_val) = mode_str.parse::<f64>() {
        Value::Double(float_val)
    } else if mode_str == "true" {
        Value::Bool(true)
    } else if mode_str == "false" {
        Value::Bool(false)
    } else {
        Value::string(mode_str)
    }
}

/// Serialize an accumulator state into a Value for inter-operator transfer.
/// The partial aggregate output stores accumulator states as Value rows.

pub(crate) fn value_list_of_values(values: &[Value]) -> Value {
    if values.is_empty() {
        Value::Null(NullType::Null)
    } else {
        Value::List(Box::new(linkrs_core::value::List::from(values.to_vec())))
    }
}

pub(crate) fn value_list_of_f64(values: &[f64]) -> Value {
    if values.is_empty() {
        Value::Null(NullType::Null)
    } else {
        Value::List(Box::new(linkrs_core::value::List::from(
            values.iter().map(|x| Value::Double(*x)).collect::<Vec<_>>(),
        )))
    }
}

/// Produce the accumulator final value from an encoded partial Value.
/// This reverses `accumulator_to_value` and calls `finalize`.

pub(crate) fn f64_list_from_value(value: &Value) -> Vec<f64> {
    match value {
        Value::List(list) => list
            .values
            .iter()
            .filter_map(|v| match v {
                Value::Double(x) => Some(*x),
                Value::Int(x) => Some(*x as f64),
                Value::BigInt(x) => Some(*x as f64),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

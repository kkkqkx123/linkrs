use std::collections::HashSet;

use super::comparison::compare_values;
use crate::core::types::operators::AggregateFunction;
use crate::core::value::NullType;
use crate::core::Value;

/// Typed accumulator state for aggregate results.
///
/// Every [`AggregateFunction`] variant has an accumulator so that the
/// streaming Aggregate, PartialAggregate and spill paths share a single
/// implementation; there is no row-based fallback.
///
/// The Welford variants (`Std`/`StddevPop`/`StddevSamp`/`Variance`) keep
/// `(n, mean, m2)` so that partial states can be merged exactly.
#[derive(Debug, Clone)]
pub enum AggregateAccumulator {
    Count(u64),
    Sum(f64),
    Min(Option<Value>),
    Max(Option<Value>),
    Avg { sum: f64, count: u64 },
    Collect(Vec<Value>),
    CollectSet(HashSet<Value>),
    Distinct(HashSet<Value>),
    Percentile { values: Vec<f64>, percentile: f64 },
    PercentileCont { values: Vec<f64>, percentile: f64 },
    Median(Vec<f64>),
    Mode(Vec<Value>),
    Std { n: u64, mean: f64, m2: f64 },
    StddevPop { n: u64, mean: f64, m2: f64 },
    StddevSamp { n: u64, mean: f64, m2: f64 },
    Variance { n: u64, mean: f64, m2: f64 },
    Product(Option<f64>),
    BitAnd(Option<i64>),
    BitOr(Option<i64>),
    BoolAnd(Option<bool>),
    BoolOr(Option<bool>),
    GroupConcat { parts: Vec<String>, separator: String },
    GroupConcatWithOrder {
        parts: Vec<String>,
        separator: String,
    },
    VecSum(Option<Vec<f32>>),
    VecAvg { sum: Vec<f32>, count: u64 },
}

impl AggregateAccumulator {
    pub fn for_function(func: &AggregateFunction) -> Option<Self> {
        match func {
            AggregateFunction::Count(_) => Some(Self::Count(0)),
            AggregateFunction::Sum(_) => Some(Self::Sum(0.0)),
            AggregateFunction::Min(_) => Some(Self::Min(None)),
            AggregateFunction::Max(_) => Some(Self::Max(None)),
            AggregateFunction::Avg(_) => Some(Self::Avg { sum: 0.0, count: 0 }),
            AggregateFunction::Collect(_) => Some(Self::Collect(Vec::new())),
            AggregateFunction::CollectSet(_) => Some(Self::CollectSet(HashSet::new())),
            AggregateFunction::Distinct(_) => Some(Self::Distinct(HashSet::new())),
            AggregateFunction::Percentile(_, p) => Some(Self::Percentile {
                values: Vec::new(),
                percentile: *p,
            }),
            AggregateFunction::PercentileCont(_, p) => Some(Self::PercentileCont {
                values: Vec::new(),
                percentile: *p,
            }),
            AggregateFunction::Median(_) => Some(Self::Median(Vec::new())),
            AggregateFunction::Mode(_) => Some(Self::Mode(Vec::new())),
            AggregateFunction::Std(_) => Some(Self::Std {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::StddevPop(_) => Some(Self::StddevPop {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::StddevSamp(_) => Some(Self::StddevSamp {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::Variance(_) => Some(Self::Variance {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::Product(_) => Some(Self::Product(None)),
            AggregateFunction::BitAnd(_) => Some(Self::BitAnd(None)),
            AggregateFunction::BitOr(_) => Some(Self::BitOr(None)),
            AggregateFunction::BoolAnd(_) => Some(Self::BoolAnd(None)),
            AggregateFunction::BoolOr(_) => Some(Self::BoolOr(None)),
            AggregateFunction::GroupConcat(_, sep) => Some(Self::GroupConcat {
                parts: Vec::new(),
                separator: sep.clone(),
            }),
            AggregateFunction::GroupConcatWithOrder(_, sep, _) => {
                Some(Self::GroupConcatWithOrder {
                    parts: Vec::new(),
                    separator: sep.clone(),
                })
            }
            AggregateFunction::VecSum(_) => Some(Self::VecSum(None)),
            AggregateFunction::VecAvg(_) => Some(Self::VecAvg {
                sum: Vec::new(),
                count: 0,
            }),
        }
    }

    /// Accumulate a single value into this state.
    pub fn accumulate(&mut self, value: &Value) {
        match self {
            Self::Count(count) => {
                if !matches!(value, Value::Null(_)) {
                    *count += 1;
                }
            }
            Self::Sum(sum) => {
                if let Some(n) = numeric_value(value) {
                    *sum += n;
                }
            }
            Self::Min(ref mut current) => {
                if matches!(value, Value::Null(_)) {
                    return;
                }
                match current {
                    None => *current = Some(value.clone()),
                    Some(c) => {
                        if compare_values(value, c).is_lt() {
                            *current = Some(value.clone());
                        }
                    }
                }
            }
            Self::Max(ref mut current) => {
                if matches!(value, Value::Null(_)) {
                    return;
                }
                match current {
                    None => *current = Some(value.clone()),
                    Some(c) => {
                        if compare_values(value, c).is_gt() {
                            *current = Some(value.clone());
                        }
                    }
                }
            }
            Self::Avg { sum, count } => {
                if let Some(n) = numeric_value(value) {
                    *sum += n;
                    *count += 1;
                }
            }
            Self::Collect(values) => {
                if !matches!(value, Value::Null(_)) {
                    values.push(value.clone());
                }
            }
            Self::CollectSet(set) | Self::Distinct(set) => {
                if !matches!(value, Value::Null(_)) {
                    set.insert(value.clone());
                }
            }
            Self::Percentile { values, .. } | Self::PercentileCont { values, .. } => {
                if let Some(n) = numeric_value(value) {
                    values.push(n);
                }
            }
            Self::Median(values) => {
                if let Some(n) = numeric_value(value) {
                    values.push(n);
                }
            }
            Self::Mode(values) => {
                if !matches!(value, Value::Null(_)) {
                    values.push(value.clone());
                }
            }
            Self::Std { n, mean, m2 }
            | Self::StddevPop { n, mean, m2 }
            | Self::StddevSamp { n, mean, m2 }
            | Self::Variance { n, mean, m2 } => {
                if let Some(x) = numeric_value(value) {
                    welford_update(n, mean, m2, x);
                }
            }
            Self::Product(product) => {
                if let Some(n) = numeric_value(value) {
                    match product {
                        None => *product = Some(n),
                        Some(p) => *p *= n,
                    }
                }
            }
            Self::BitAnd(current) => {
                if let Value::BigInt(v) = value {
                    *current = Some(current.unwrap_or(i64::MAX) & v);
                }
            }
            Self::BitOr(current) => {
                if let Value::BigInt(v) = value {
                    *current = Some(current.unwrap_or(0) | v);
                }
            }
            Self::BoolAnd(current) => {
                if let Value::Bool(b) = value {
                    *current = Some(current.unwrap_or(true) && *b);
                }
            }
            Self::BoolOr(current) => {
                if let Value::Bool(b) = value {
                    *current = Some(current.unwrap_or(false) || *b);
                }
            }
            Self::GroupConcat { parts, .. } | Self::GroupConcatWithOrder { parts, .. } => {
                if !matches!(value, Value::Null(_)) {
                    parts.push(format!("{}", value));
                }
            }
            Self::VecSum(sum) => {
                if let Value::Vector(vec) = value {
                    let dense = vec.to_dense();
                    match sum {
                        None => *sum = Some(dense),
                        Some(s) => {
                            if s.len() == dense.len() {
                                for (a, b) in s.iter_mut().zip(dense) {
                                    *a += b;
                                }
                            }
                        }
                    }
                }
            }
            Self::VecAvg { sum, count } => {
                if let Value::Vector(vec) = value {
                    let dense = vec.to_dense();
                    *count += 1;
                    if sum.is_empty() {
                        *sum = dense;
                    } else if sum.len() == dense.len() {
                        for (x, y) in sum.iter_mut().zip(dense) {
                            *x += y;
                        }
                    }
                }
            }
        }
    }

    /// Merge another accumulator of the same kind into this one.
    pub fn merge(&mut self, other: &AggregateAccumulator) {
        match (self, other) {
            (Self::Count(a), Self::Count(b)) => *a += b,
            (Self::Sum(a), Self::Sum(b)) => *a += b,
            (Self::Min(ref mut a), Self::Min(b)) => match (a.as_ref(), b) {
                (None, Some(v)) => *a = Some(v.clone()),
                (Some(_), None) => {}
                (Some(a_val), Some(b_val)) => {
                    if compare_values(b_val, a_val).is_lt() {
                        *a = Some(b_val.clone());
                    }
                }
                (None, None) => {}
            },
            (Self::Max(ref mut a), Self::Max(b)) => match (a.as_ref(), b) {
                (None, Some(v)) => *a = Some(v.clone()),
                (Some(_), None) => {}
                (Some(a_val), Some(b_val)) => {
                    if compare_values(b_val, a_val).is_gt() {
                        *a = Some(b_val.clone());
                    }
                }
                (None, None) => {}
            },
            (Self::Avg { sum: s1, count: c1 }, Self::Avg { sum: s2, count: c2 }) => {
                *s1 += s2;
                *c1 += c2;
            }
            (Self::Collect(a), Self::Collect(b)) => a.extend_from_slice(b),
            (Self::CollectSet(a), Self::CollectSet(b))
            | (Self::Distinct(a), Self::Distinct(b)) => a.extend(b.iter().cloned()),
            (Self::Percentile { values: a, .. }, Self::Percentile { values: b, .. })
            | (
                Self::PercentileCont { values: a, .. },
                Self::PercentileCont { values: b, .. },
            ) => a.extend_from_slice(b),
            (Self::Median(a), Self::Median(b)) => a.extend_from_slice(b),
            (Self::Mode(a), Self::Mode(b)) => a.extend_from_slice(b),
            (
                Self::Std { n: n1, mean: m1, m2: v1 },
                Self::Std { n: n2, mean: m2, m2: v2 },
            )
            | (
                Self::StddevPop { n: n1, mean: m1, m2: v1 },
                Self::StddevPop { n: n2, mean: m2, m2: v2 },
            )
            | (
                Self::StddevSamp { n: n1, mean: m1, m2: v1 },
                Self::StddevSamp { n: n2, mean: m2, m2: v2 },
            )
            | (
                Self::Variance { n: n1, mean: m1, m2: v1 },
                Self::Variance { n: n2, mean: m2, m2: v2 },
            ) => welford_merge(n1, m1, v1, n2, m2, v2),
            (Self::Product(a), Self::Product(b)) => {
                if let (Some(x), Some(y)) = (*a, *b) {
                    *a = Some(x * y);
                } else if a.is_none() {
                    *a = *b;
                }
            }
            (Self::BitAnd(a), Self::BitAnd(b)) => {
                if let (Some(x), Some(y)) = (*a, *b) {
                    *a = Some(x & y);
                } else if a.is_none() {
                    *a = *b;
                }
            }
            (Self::BitOr(a), Self::BitOr(b)) => {
                if let (Some(x), Some(y)) = (*a, *b) {
                    *a = Some(x | y);
                } else if a.is_none() {
                    *a = *b;
                }
            }
            (Self::BoolAnd(a), Self::BoolAnd(b)) => {
                if let (Some(x), Some(y)) = (*a, *b) {
                    *a = Some(x && y);
                } else if a.is_none() {
                    *a = *b;
                }
            }
            (Self::BoolOr(a), Self::BoolOr(b)) => {
                if let (Some(x), Some(y)) = (*a, *b) {
                    *a = Some(x || y);
                } else if a.is_none() {
                    *a = *b;
                }
            }
            (Self::GroupConcat { parts: a, .. }, Self::GroupConcat { parts: b, .. })
            | (
                Self::GroupConcatWithOrder { parts: a, .. },
                Self::GroupConcatWithOrder { parts: b, .. },
            ) => a.extend_from_slice(b),
            (Self::VecSum(a), Self::VecSum(b)) => {
                if let (Some(x), Some(y)) = (a.as_mut(), b.as_ref()) {
                    if x.len() == y.len() {
                        for (xi, yi) in x.iter_mut().zip(y) {
                            *xi += yi;
                        }
                    }
                } else if a.is_none() {
                    *a = b.clone();
                }
            }
            (
                Self::VecAvg { sum: s1, count: c1 },
                Self::VecAvg { sum: s2, count: c2 },
            ) => {
                if s1.is_empty() {
                    *s1 = s2.clone();
                } else if s1.len() == s2.len() {
                    for (x, y) in s1.iter_mut().zip(s2) {
                        *x += y;
                    }
                }
                *c1 += c2;
            }
            _ => {}
        }
    }

    /// Produce the final aggregate Value from this accumulator.
    pub fn finalize(&self) -> Value {
        match self {
            Self::Count(count) => Value::BigInt(*count as i64),
            Self::Sum(sum) => integral_or_double(*sum),
            Self::Min(Some(v)) | Self::Max(Some(v)) => v.clone(),
            Self::Min(None) | Self::Max(None) => Value::Null(NullType::Null),
            Self::Avg { sum, count } => {
                if *count == 0 {
                    Value::Null(NullType::Null)
                } else {
                    Value::Double(*sum / *count as f64)
                }
            }
            Self::Collect(values) => {
                if values.is_empty() {
                    Value::Null(NullType::Null)
                } else {
                    let mut list = crate::core::value::List::new();
                    for v in values {
                        list.push(v.clone());
                    }
                    Value::List(Box::new(list))
                }
            }
            Self::CollectSet(set) | Self::Distinct(set) => {
                if set.is_empty() {
                    Value::Null(NullType::Null)
                } else {
                    Value::set(set.clone())
                }
            }
            Self::Percentile { values, percentile }
            | Self::PercentileCont { values, percentile } => {
                percentile_of(values, *percentile)
            }
            Self::Median(values) => median_of(values),
            Self::Mode(values) => mode_of(values),
            Self::Std { n, mean: _, m2 } | Self::Variance { n, mean: _, m2 } => {
                if *n == 0 {
                    Value::Null(NullType::Null)
                } else if matches!(self, Self::Std { .. }) {
                    Value::Double((m2 / *n as f64).sqrt())
                } else {
                    Value::Double(m2 / *n as f64)
                }
            }
            Self::StddevPop { n, mean: _, m2 } => {
                if *n == 0 {
                    Value::Null(NullType::Null)
                } else {
                    Value::Double((m2 / *n as f64).sqrt())
                }
            }
            Self::StddevSamp { n, mean: _, m2 } => {
                if *n < 2 {
                    Value::Null(NullType::Null)
                } else {
                    Value::Double((m2 / (*n - 1) as f64).sqrt())
                }
            }
            Self::Product(None) => Value::BigInt(0),
            Self::Product(Some(p)) => integral_or_double(*p),
            Self::BitAnd(Some(v)) | Self::BitOr(Some(v)) => Value::BigInt(*v),
            Self::BitAnd(None) | Self::BitOr(None) => Value::Null(NullType::Null),
            Self::BoolAnd(Some(v)) | Self::BoolOr(Some(v)) => Value::Bool(*v),
            Self::BoolAnd(None) | Self::BoolOr(None) => Value::Null(NullType::Null),
            Self::GroupConcat { parts, separator }
            | Self::GroupConcatWithOrder { parts, separator } => {
                Value::string(parts.join(separator))
            }
            Self::VecSum(Some(vec)) => Value::vector(vec.clone()),
            Self::VecSum(None) => Value::Null(NullType::NaN),
            Self::VecAvg { sum, count } => {
                if *count == 0 {
                    Value::Null(NullType::NaN)
                } else {
                    Value::vector(sum.iter().map(|x| x / *count as f32).collect())
                }
            }
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Count(_) => "Count",
            Self::Sum(_) => "Sum",
            Self::Min(_) => "Min",
            Self::Max(_) => "Max",
            Self::Avg { .. } => "Avg",
            Self::Collect(_) => "Collect",
            Self::CollectSet(_) => "CollectSet",
            Self::Distinct(_) => "Distinct",
            Self::Percentile { .. } => "Percentile",
            Self::PercentileCont { .. } => "PercentileCont",
            Self::Median(_) => "Median",
            Self::Mode(_) => "Mode",
            Self::Std { .. } => "Std",
            Self::StddevPop { .. } => "StddevPop",
            Self::StddevSamp { .. } => "StddevSamp",
            Self::Variance { .. } => "Variance",
            Self::Product(_) => "Product",
            Self::BitAnd(_) => "BitAnd",
            Self::BitOr(_) => "BitOr",
            Self::BoolAnd(_) => "BoolAnd",
            Self::BoolOr(_) => "BoolOr",
            Self::GroupConcat { .. } => "GroupConcat",
            Self::GroupConcatWithOrder { .. } => "GroupConcatWithOrder",
            Self::VecSum(_) => "VecSum",
            Self::VecAvg { .. } => "VecAvg",
        }
    }
}

fn numeric_value(value: &Value) -> Option<f64> {
    match value {
        Value::Int(n) => Some(*n as f64),
        Value::BigInt(n) => Some(*n as f64),
        Value::Float(f) => Some(*f as f64),
        Value::Double(f) => Some(*f),
        _ => None,
    }
}

fn integral_or_double(value: f64) -> Value {
    if value.fract() == 0.0 && value.is_finite() {
        Value::BigInt(value as i64)
    } else {
        Value::Double(value)
    }
}

fn welford_update(n: &mut u64, mean: &mut f64, m2: &mut f64, x: f64) {
    *n += 1;
    let delta = x - *mean;
    *mean += delta / *n as f64;
    let delta2 = x - *mean;
    *m2 += delta * delta2;
}

fn welford_merge(n1: &mut u64, mean1: &mut f64, m21: &mut f64, n2: &u64, mean2: &f64, m22: &f64) {
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

fn percentile_of(values: &[f64], percentile: f64) -> Value {
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

fn median_of(values: &[f64]) -> Value {
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

fn mode_of(values: &[Value]) -> Value {
    if values.is_empty() {
        return Value::Null(NullType::Null);
    }
    let mut frequency: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for value in values {
        *frequency.entry(format!("{}", value)).or_insert(0) += 1;
    }
    let Some(mode_str) = frequency.into_iter().max_by_key(|(_, count)| *count)
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
pub fn accumulator_to_value(acc: &AggregateAccumulator) -> Value {
    match acc {
        AggregateAccumulator::Count(c) => Value::BigInt(*c as i64),
        AggregateAccumulator::Sum(s) => Value::Double(*s),
        AggregateAccumulator::Min(Some(v)) | AggregateAccumulator::Max(Some(v)) => v.clone(),
        AggregateAccumulator::Min(None) | AggregateAccumulator::Max(None) => {
            Value::Null(NullType::Null)
        }
        AggregateAccumulator::Avg { sum, count } => {
            let mut list = crate::core::value::List::new();
            list.push(Value::Double(*sum));
            list.push(Value::BigInt(*count as i64));
            Value::List(Box::new(list))
        }
        AggregateAccumulator::Collect(values) => value_list_of_values(values),
        AggregateAccumulator::CollectSet(set) | AggregateAccumulator::Distinct(set) => {
            if set.is_empty() {
                Value::Null(NullType::Null)
            } else {
                Value::List(Box::new(crate::core::value::List::from(
                    set.iter().cloned().collect::<Vec<_>>(),
                )))
            }
        }
        AggregateAccumulator::Percentile { values, .. }
        | AggregateAccumulator::PercentileCont { values, .. }
        | AggregateAccumulator::Median(values) => value_list_of_f64(values),
        AggregateAccumulator::Mode(values) => value_list_of_values(values),
        AggregateAccumulator::Std { n, mean, m2 }
        | AggregateAccumulator::StddevPop { n, mean, m2 }
        | AggregateAccumulator::StddevSamp { n, mean, m2 }
        | AggregateAccumulator::Variance { n, mean, m2 } => {
            if *n == 0 {
                Value::Null(NullType::Null)
            } else {
                let mut list = crate::core::value::List::new();
                list.push(Value::BigInt(*n as i64));
                list.push(Value::Double(*mean));
                list.push(Value::Double(*m2));
                Value::List(Box::new(list))
            }
        }
        AggregateAccumulator::Product(Some(p)) => Value::Double(*p),
        AggregateAccumulator::Product(None) => Value::Null(NullType::Null),
        AggregateAccumulator::BitAnd(Some(v)) | AggregateAccumulator::BitOr(Some(v)) => {
            Value::BigInt(*v)
        }
        AggregateAccumulator::BitAnd(None) | AggregateAccumulator::BitOr(None) => {
            Value::Null(NullType::Null)
        }
        AggregateAccumulator::BoolAnd(Some(v)) | AggregateAccumulator::BoolOr(Some(v)) => {
            Value::Bool(*v)
        }
        AggregateAccumulator::BoolAnd(None) | AggregateAccumulator::BoolOr(None) => {
            Value::Null(NullType::Null)
        }
        AggregateAccumulator::GroupConcat { parts, .. }
        | AggregateAccumulator::GroupConcatWithOrder { parts, .. } => {
            if parts.is_empty() {
                Value::Null(NullType::Null)
            } else {
                Value::List(Box::new(crate::core::value::List::from(
                    parts.iter().cloned().map(Value::string).collect::<Vec<_>>(),
                )))
            }
        }
        AggregateAccumulator::VecSum(Some(vec)) => value_list_of_f64(
            &vec.iter().map(|x| *x as f64).collect::<Vec<_>>(),
        ),
        AggregateAccumulator::VecSum(None) => Value::Null(NullType::Null),
        AggregateAccumulator::VecAvg { sum, count } => {
            if *count == 0 {
                Value::Null(NullType::Null)
            } else {
                let mut list = crate::core::value::List::new();
                let mut inner = crate::core::value::List::new();
                for x in sum {
                    inner.push(Value::Double(*x as f64));
                }
                list.push(Value::List(Box::new(inner)));
                list.push(Value::BigInt(*count as i64));
                Value::List(Box::new(list))
            }
        }
    }
}

fn value_list_of_values(values: &[Value]) -> Value {
    if values.is_empty() {
        Value::Null(NullType::Null)
    } else {
        Value::List(Box::new(crate::core::value::List::from(
            values.to_vec(),
        )))
    }
}

fn value_list_of_f64(values: &[f64]) -> Value {
    if values.is_empty() {
        Value::Null(NullType::Null)
    } else {
        Value::List(Box::new(crate::core::value::List::from(
            values.iter().map(|x| Value::Double(*x)).collect::<Vec<_>>(),
        )))
    }
}

/// Produce the accumulator final value from an encoded partial Value.
/// This reverses `accumulator_to_value` and calls `finalize`.
pub fn finalize_accumulator_value(
    func: &AggregateFunction,
    partial_value: &Value,
    _count_accumulator: Option<&AggregateAccumulator>,
) -> Value {
    match decode_partial(func, partial_value) {
        Some(acc) => acc.finalize(),
        None => partial_value.clone(),
    }
}

/// Decode a partial-accumulator Value back into an accumulator state.
/// This reverses `accumulator_to_value`.
pub fn decode_partial(func: &AggregateFunction, value: &Value) -> Option<AggregateAccumulator> {
    let mut acc = AggregateAccumulator::for_function(func)?;
    match &mut acc {
        AggregateAccumulator::Count(c) => {
            if let Value::BigInt(n) = value {
                *c = *n as u64;
            }
        }
        AggregateAccumulator::Sum(s) => {
            if let Value::Double(n) = value {
                *s = *n;
            } else if let Value::BigInt(n) = value {
                *s = *n as f64;
            }
        }
        AggregateAccumulator::Min(ref mut v) | AggregateAccumulator::Max(ref mut v) => {
            if !matches!(value, Value::Null(_)) {
                *v = Some(value.clone());
            }
        }
        AggregateAccumulator::Avg { sum, count } => {
            if let Value::List(list) = value {
                if list.len() >= 2 {
                    if let Value::Double(s) = &list.values[0] {
                        *sum = *s;
                    }
                    if let Value::BigInt(c) = &list.values[1] {
                        *count = *c as u64;
                    }
                }
            }
        }
        AggregateAccumulator::Collect(values) => {
            if let Value::List(list) = value {
                *values = list.values.clone();
            }
        }
        AggregateAccumulator::CollectSet(set) | AggregateAccumulator::Distinct(set) => {
            if let Value::List(list) = value {
                *set = list.values.iter().cloned().collect();
            }
        }
        AggregateAccumulator::Percentile { values, .. }
        | AggregateAccumulator::PercentileCont { values, .. }
        | AggregateAccumulator::Median(values) => {
            *values = f64_list_from_value(value);
        }
        AggregateAccumulator::Mode(values) => {
            if let Value::List(list) = value {
                *values = list.values.clone();
            }
        }
        AggregateAccumulator::Std { n, mean, m2 }
        | AggregateAccumulator::StddevPop { n, mean, m2 }
        | AggregateAccumulator::StddevSamp { n, mean, m2 }
        | AggregateAccumulator::Variance { n, mean, m2 } => {
            if let Value::List(list) = value {
                if list.len() >= 3 {
                    if let Value::BigInt(nn) = &list.values[0] {
                        *n = *nn as u64;
                    }
                    if let Value::Double(mm) = &list.values[1] {
                        *mean = *mm;
                    }
                    if let Value::Double(mm2) = &list.values[2] {
                        *m2 = *mm2;
                    }
                }
            }
        }
        AggregateAccumulator::Product(p) => match value {
            Value::Double(n) => *p = Some(*n),
            Value::BigInt(n) => *p = Some(*n as f64),
            _ => *p = None,
        },
        AggregateAccumulator::BitAnd(v) | AggregateAccumulator::BitOr(v) => {
            if let Value::BigInt(n) = value {
                *v = Some(*n);
            }
        }
        AggregateAccumulator::BoolAnd(v) | AggregateAccumulator::BoolOr(v) => {
            if let Value::Bool(b) = value {
                *v = Some(*b);
            }
        }
        AggregateAccumulator::GroupConcat { parts, .. }
        | AggregateAccumulator::GroupConcatWithOrder { parts, .. } => {
            if let Value::List(list) = value {
                *parts = list
                    .values
                    .iter()
                    .map(|v| format!("{}", v))
                    .collect();
            }
        }
        AggregateAccumulator::VecSum(sum) => match value {
            Value::List(list) => {
                *sum = Some(
                    list.values
                        .iter()
                        .filter_map(|v| match v {
                            Value::Double(x) => Some(*x as f32),
                            Value::Int(x) => Some(*x as f32),
                            Value::BigInt(x) => Some(*x as f32),
                            _ => None,
                        })
                        .collect(),
                );
            }
            _ => *sum = None,
        },
        AggregateAccumulator::VecAvg { sum, count } => {
            if let Value::List(list) = value {
                if list.len() >= 2 {
                    if let Value::List(inner) = &list.values[0] {
                        *sum = inner
                            .values
                            .iter()
                            .filter_map(|v| match v {
                                Value::Double(x) => Some(*x as f32),
                                Value::Int(x) => Some(*x as f32),
                                Value::BigInt(x) => Some(*x as f32),
                                _ => None,
                            })
                            .collect();
                    }
                    if let Value::BigInt(c) = &list.values[1] {
                        *count = *c as u64;
                    }
                }
            }
        }
    }
    Some(acc)
}

fn f64_list_from_value(value: &Value) -> Vec<f64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_accumulate() {
        let mut acc = AggregateAccumulator::Count(0);
        acc.accumulate(&Value::Int(1));
        acc.accumulate(&Value::Int(2));
        acc.accumulate(&Value::Null(NullType::Null));
        assert_eq!(acc.finalize(), Value::BigInt(2));
    }

    #[test]
    fn test_sum_accumulate() {
        let mut acc = AggregateAccumulator::Sum(0.0);
        acc.accumulate(&Value::Int(1));
        acc.accumulate(&Value::BigInt(2));
        acc.accumulate(&Value::Double(3.5));
        acc.accumulate(&Value::Null(NullType::Null));
        assert!(
            (match acc.finalize() {
                Value::Double(d) => (d - 6.5).abs() < 1e-10,
                _ => false,
            })
        );
    }

    #[test]
    fn test_min_max_typed_compare() {
        // Type-aware comparison: Int 9 < Int 10 despite "10" < "9" as strings.
        let mut min_acc = AggregateAccumulator::Min(None);
        let mut max_acc = AggregateAccumulator::Max(None);
        for v in [Value::Int(10), Value::Int(9), Value::Int(11)] {
            min_acc.accumulate(&v);
            max_acc.accumulate(&v);
        }
        assert_eq!(min_acc.finalize(), Value::Int(9));
        assert_eq!(max_acc.finalize(), Value::Int(11));
    }

    #[test]
    fn test_avg_accumulate() {
        let mut acc = AggregateAccumulator::Avg { sum: 0.0, count: 0 };
        acc.accumulate(&Value::Int(2));
        acc.accumulate(&Value::Int(4));
        acc.accumulate(&Value::Int(6));
        assert!(
            (match acc.finalize() {
                Value::Double(d) => (d - 4.0).abs() < 1e-10,
                _ => false,
            })
        );
    }

    #[test]
    fn test_merge_counts() {
        let mut a = AggregateAccumulator::Count(3);
        let b = AggregateAccumulator::Count(5);
        a.merge(&b);
        assert_eq!(a.finalize(), Value::BigInt(8));
    }

    #[test]
    fn test_merge_avg() {
        let mut a = AggregateAccumulator::Avg {
            sum: 10.0,
            count: 3,
        };
        let b = AggregateAccumulator::Avg {
            sum: 20.0,
            count: 2,
        };
        a.merge(&b);
        assert!(
            (match a.finalize() {
                Value::Double(d) => (d - 6.0).abs() < 1e-10,
                _ => false,
            })
        );
    }

    #[test]
    fn test_empty_returns_null() {
        let min = AggregateAccumulator::Min(None);
        let max = AggregateAccumulator::Max(None);
        let avg = AggregateAccumulator::Avg { sum: 0.0, count: 0 };
        assert_eq!(min.finalize(), Value::Null(NullType::Null));
        assert_eq!(max.finalize(), Value::Null(NullType::Null));
        assert_eq!(avg.finalize(), Value::Null(NullType::Null));
    }

    #[test]
    fn test_collect() {
        let mut acc = AggregateAccumulator::Collect(Vec::new());
        acc.accumulate(&Value::Int(1));
        acc.accumulate(&Value::Null(NullType::Null));
        acc.accumulate(&Value::Int(2));
        match acc.finalize() {
            Value::List(l) => {
                assert_eq!(l.values, vec![Value::Int(1), Value::Int(2)]);
            }
            other => panic!("expected list, got {:?}", other),
        }
    }

    #[test]
    fn test_collect_set() {
        let mut acc = AggregateAccumulator::CollectSet(HashSet::new());
        acc.accumulate(&Value::Int(1));
        acc.accumulate(&Value::Int(1));
        acc.accumulate(&Value::Int(2));
        match acc.finalize() {
            Value::Set(s) => assert_eq!(s.len(), 2),
            other => panic!("expected set, got {:?}", other),
        }
    }

    #[test]
    fn test_percentile_and_median() {
        let mut p = AggregateAccumulator::Percentile {
            values: Vec::new(),
            percentile: 50.0,
        };
        for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
            p.accumulate(&v);
        }
        assert!(
            (match p.finalize() {
                Value::Double(d) => (d - 2.5).abs() < 1e-9,
                _ => false,
            })
        );

        let mut m = AggregateAccumulator::Median(Vec::new());
        for v in [Value::Int(1), Value::Int(2), Value::Int(3)] {
            m.accumulate(&v);
        }
        assert!(
            (match m.finalize() {
                Value::Double(d) => (d - 2.0).abs() < 1e-9,
                _ => false,
            })
        );
    }

    #[test]
    fn test_std_variance_family() {
        let mut std = AggregateAccumulator::Std {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
            std.accumulate(&v);
        }
        // Population std of [1,2,3,4] = sqrt(1.25)
        assert!(
            (match std.finalize() {
                Value::Double(d) => (d - 1.25f64.sqrt()).abs() < 1e-9,
                _ => false,
            })
        );

        let mut samp = AggregateAccumulator::StddevSamp {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
            samp.accumulate(&v);
        }
        // Sample std of [1,2,3,4] = sqrt(5/3)
        assert!(
            (match samp.finalize() {
                Value::Double(d) => (d - (5.0f64 / 3.0).sqrt()).abs() < 1e-9,
                _ => false,
            })
        );

        let mut var = AggregateAccumulator::Variance {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        for v in [Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(4)] {
            var.accumulate(&v);
        }
        assert!(
            (match var.finalize() {
                Value::Double(d) => (d - 1.25).abs() < 1e-9,
                _ => false,
            })
        );

        let empty = AggregateAccumulator::StddevSamp {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        assert_eq!(empty.finalize(), Value::Null(NullType::Null));
    }

    #[test]
    fn test_welford_merge_matches_single_pass() {
        let mut a = AggregateAccumulator::Std {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        let mut b = AggregateAccumulator::Std {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        for v in [Value::Int(1), Value::Int(3)] {
            a.accumulate(&v);
        }
        for v in [Value::Int(2), Value::Int(4), Value::Int(5)] {
            b.accumulate(&v);
        }
        let mut merged = AggregateAccumulator::Std {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        let mut single = AggregateAccumulator::Std {
            n: 0,
            mean: 0.0,
            m2: 0.0,
        };
        merged.merge(&a);
        merged.merge(&b);
        for v in [Value::Int(1), Value::Int(3), Value::Int(2), Value::Int(4), Value::Int(5)] {
            single.accumulate(&v);
        }
        let (Value::Double(md), Value::Double(sd)) = (merged.finalize(), single.finalize()) else {
            panic!("expected doubles");
        };
        assert!((md - sd).abs() < 1e-9);
    }

    #[test]
    fn test_product() {
        let mut acc = AggregateAccumulator::Product(None);
        acc.accumulate(&Value::Int(2));
        acc.accumulate(&Value::Int(3));
        acc.accumulate(&Value::Null(NullType::Null));
        assert_eq!(acc.finalize(), Value::BigInt(6));
        let empty = AggregateAccumulator::Product(None);
        assert_eq!(empty.finalize(), Value::BigInt(0));
    }

    #[test]
    fn test_bit_bool() {
        let mut and = AggregateAccumulator::BitAnd(None);
        and.accumulate(&Value::BigInt(0b1100));
        and.accumulate(&Value::BigInt(0b1010));
        assert_eq!(and.finalize(), Value::BigInt(0b1000));

        let mut band = AggregateAccumulator::BoolAnd(None);
        band.accumulate(&Value::Bool(true));
        band.accumulate(&Value::Bool(true));
        assert_eq!(band.finalize(), Value::Bool(true));
        band.accumulate(&Value::Bool(false));
        assert_eq!(band.finalize(), Value::Bool(false));

        let mut bor = AggregateAccumulator::BoolOr(None);
        bor.accumulate(&Value::Bool(false));
        bor.accumulate(&Value::Bool(true));
        assert_eq!(bor.finalize(), Value::Bool(true));
    }

    #[test]
    fn test_group_concat() {
        let mut acc = AggregateAccumulator::GroupConcat {
            parts: Vec::new(),
            separator: ", ".to_string(),
        };
        acc.accumulate(&Value::string("a"));
        acc.accumulate(&Value::string("b"));
        assert_eq!(acc.finalize(), Value::string("a, b"));
    }

    #[test]
    fn test_vec_sum_avg() {
        let mut sum = AggregateAccumulator::VecSum(None);
        sum.accumulate(&Value::vector(vec![1.0, 2.0]));
        sum.accumulate(&Value::vector(vec![3.0, 4.0]));
        match sum.finalize() {
            Value::Vector(v) => {
                assert_eq!(v.to_dense(), vec![4.0, 6.0]);
            }
            other => panic!("expected vector, got {:?}", other),
        }

        let mut avg = AggregateAccumulator::VecAvg {
            sum: Vec::new(),
            count: 0,
        };
        avg.accumulate(&Value::vector(vec![1.0, 3.0]));
        avg.accumulate(&Value::vector(vec![3.0, 5.0]));
        match avg.finalize() {
            Value::Vector(v) => {
                assert_eq!(v.to_dense(), vec![2.0, 4.0]);
            }
            other => panic!("expected vector, got {:?}", other),
        }
    }

    #[test]
    fn test_for_function_mapping_covers_all_variants() {
        let funcs = [
            AggregateFunction::Count(None),
            AggregateFunction::Sum("x".to_string()),
            AggregateFunction::Min("x".to_string()),
            AggregateFunction::Max("x".to_string()),
            AggregateFunction::Avg("x".to_string()),
            AggregateFunction::Collect("x".to_string()),
            AggregateFunction::CollectSet("x".to_string()),
            AggregateFunction::Distinct("x".to_string()),
            AggregateFunction::Percentile("x".to_string(), 50.0),
            AggregateFunction::Std("x".to_string()),
            AggregateFunction::StddevPop("x".to_string()),
            AggregateFunction::StddevSamp("x".to_string()),
            AggregateFunction::Variance("x".to_string()),
            AggregateFunction::Product("x".to_string()),
            AggregateFunction::PercentileCont("x".to_string(), 50.0),
            AggregateFunction::Median("x".to_string()),
            AggregateFunction::Mode("x".to_string()),
            AggregateFunction::BitAnd("x".to_string()),
            AggregateFunction::BitOr("x".to_string()),
            AggregateFunction::BoolAnd("x".to_string()),
            AggregateFunction::BoolOr("x".to_string()),
            AggregateFunction::GroupConcat("x".to_string(), ",".to_string()),
            AggregateFunction::GroupConcatWithOrder(
                "x".to_string(),
                ",".to_string(),
                Vec::new(),
            ),
            AggregateFunction::VecSum("x".to_string()),
            AggregateFunction::VecAvg("x".to_string()),
        ];
        for func in &funcs {
            assert!(
                AggregateAccumulator::for_function(func).is_some(),
                "missing accumulator for {:?}",
                func
            );
        }
    }

    #[test]
    fn test_accumulator_to_value_roundtrip() {
        let acc = AggregateAccumulator::Avg {
            sum: 15.0,
            count: 3,
        };
        let v = accumulator_to_value(&acc);
        let result = finalize_accumulator_value(&AggregateFunction::Avg("x".to_string()), &v, None);
        assert!(
            (match result {
                Value::Double(d) => (d - 5.0).abs() < 1e-10,
                _ => false,
            })
        );
    }

    #[test]
    fn test_decode_partial_roundtrip_all_variants() {
        let cases: Vec<(AggregateFunction, AggregateAccumulator)> = vec![
            (
                AggregateFunction::Count(None),
                AggregateAccumulator::Count(7),
            ),
            (
                AggregateFunction::Sum("x".to_string()),
                AggregateAccumulator::Sum(3.5),
            ),
            (
                AggregateFunction::Min("x".to_string()),
                AggregateAccumulator::Min(Some(Value::Int(3))),
            ),
            (
                AggregateFunction::Max("x".to_string()),
                AggregateAccumulator::Max(Some(Value::Int(9))),
            ),
            (
                AggregateFunction::Avg("x".to_string()),
                AggregateAccumulator::Avg { sum: 4.0, count: 2 },
            ),
            (
                AggregateFunction::Collect("x".to_string()),
                AggregateAccumulator::Collect(vec![Value::Int(1), Value::Int(2)]),
            ),
            (
                AggregateFunction::CollectSet("x".to_string()),
                AggregateAccumulator::CollectSet(
                    [Value::Int(1), Value::Int(2)].into_iter().collect(),
                ),
            ),
            (
                AggregateFunction::Distinct("x".to_string()),
                AggregateAccumulator::Distinct([Value::Int(1)].into_iter().collect()),
            ),
            (
                AggregateFunction::Percentile("x".to_string(), 90.0),
                AggregateAccumulator::Percentile {
                    values: vec![1.0, 2.0, 3.0],
                    percentile: 90.0,
                },
            ),
            (
                AggregateFunction::Median("x".to_string()),
                AggregateAccumulator::Median(vec![1.0, 2.0]),
            ),
            (
                AggregateFunction::Mode("x".to_string()),
                AggregateAccumulator::Mode(vec![Value::Int(1), Value::Int(1)]),
            ),
            (
                AggregateFunction::Std("x".to_string()),
                AggregateAccumulator::Std {
                    n: 3,
                    mean: 2.0,
                    m2: 2.0,
                },
            ),
            (
                AggregateFunction::StddevSamp("x".to_string()),
                AggregateAccumulator::StddevSamp {
                    n: 4,
                    mean: 1.0,
                    m2: 1.5,
                },
            ),
            (
                AggregateFunction::Variance("x".to_string()),
                AggregateAccumulator::Variance {
                    n: 2,
                    mean: 0.5,
                    m2: 0.25,
                },
            ),
            (
                AggregateFunction::Product("x".to_string()),
                AggregateAccumulator::Product(Some(6.0)),
            ),
            (
                AggregateFunction::BitAnd("x".to_string()),
                AggregateAccumulator::BitAnd(Some(4)),
            ),
            (
                AggregateFunction::BoolAnd("x".to_string()),
                AggregateAccumulator::BoolAnd(Some(true)),
            ),
            (
                AggregateFunction::GroupConcat("x".to_string(), ";".to_string()),
                AggregateAccumulator::GroupConcat {
                    parts: vec!["a".to_string(), "b".to_string()],
                    separator: ";".to_string(),
                },
            ),
            (
                AggregateFunction::VecSum("x".to_string()),
                AggregateAccumulator::VecSum(Some(vec![1.0, 2.0])),
            ),
            (
                AggregateFunction::VecAvg("x".to_string()),
                AggregateAccumulator::VecAvg {
                    sum: vec![2.0, 4.0],
                    count: 2,
                },
            ),
        ];
        for (func, acc) in cases {
            let encoded = accumulator_to_value(&acc);
            let decoded = decode_partial(&func, &encoded).expect("decode should succeed");
            let mut merged = AggregateAccumulator::for_function(&func).unwrap();
            merged.merge(&decoded);
            assert_eq!(
                merged.finalize(),
                acc.finalize(),
                "roundtrip mismatch for {:?}",
                func
            );
        }
    }
}

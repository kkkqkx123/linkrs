use std::collections::HashSet;

use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use super::super::comparison::compare_values;
use super::stats::{
    integral_or_double, median_of, mode_of, numeric_value, percentile_of, welford_merge,
    welford_update,
};

/// Extract a numeric parameter (e.g. the percentile fraction) from the
/// aggregate's `args` list. Parameters beyond the field expression live at
/// index 1 (index 0 is the aggregated field itself).
fn aggregate_param_f64(args: &[Expression], idx: usize) -> Option<f64> {
    match args.get(idx) {
        Some(Expression::Literal(Value::Int(v))) => Some(*v as f64),
        Some(Expression::Literal(Value::BigInt(v))) => Some(*v as f64),
        Some(Expression::Literal(Value::Float(v))) => Some(*v as f64),
        Some(Expression::Literal(Value::Double(v))) => Some(*v),
        _ => None,
    }
}

/// Extract a string parameter (e.g. the GROUP_CONCAT separator) from the
/// aggregate's `args` list (index 1).
fn aggregate_param_string(args: &[Expression], idx: usize) -> Option<String> {
    match args.get(idx) {
        Some(Expression::Literal(Value::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

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
    Avg {
        sum: f64,
        count: u64,
    },
    Collect(Vec<Value>),
    CollectSet(HashSet<Value>),
    Distinct(HashSet<Value>),
    Percentile {
        values: Vec<f64>,
        percentile: f64,
    },
    PercentileCont {
        values: Vec<f64>,
        percentile: f64,
    },
    Median(Vec<f64>),
    Mode(Vec<Value>),
    Std {
        n: u64,
        mean: f64,
        m2: f64,
    },
    StddevPop {
        n: u64,
        mean: f64,
        m2: f64,
    },
    StddevSamp {
        n: u64,
        mean: f64,
        m2: f64,
    },
    Variance {
        n: u64,
        mean: f64,
        m2: f64,
    },
    Product(Option<f64>),
    BitAnd(Option<i64>),
    BitOr(Option<i64>),
    BoolAnd(Option<bool>),
    BoolOr(Option<bool>),
    GroupConcat {
        parts: Vec<String>,
        separator: String,
    },
    GroupConcatWithOrder {
        parts: Vec<String>,
        separator: String,
    },
    VecSum(Option<Vec<f32>>),
    VecAvg {
        sum: Vec<f32>,
        count: u64,
    },
}

impl AggregateAccumulator {
    pub fn for_function(func: &AggregateFunction, args: &[Expression]) -> Option<Self> {
        match func {
            AggregateFunction::Count => Some(Self::Count(0)),
            AggregateFunction::Sum => Some(Self::Sum(0.0)),
            AggregateFunction::Min => Some(Self::Min(None)),
            AggregateFunction::Max => Some(Self::Max(None)),
            AggregateFunction::Avg => Some(Self::Avg { sum: 0.0, count: 0 }),
            AggregateFunction::Collect => Some(Self::Collect(Vec::new())),
            AggregateFunction::CollectSet => Some(Self::CollectSet(HashSet::new())),
            AggregateFunction::Percentile => Some(Self::Percentile {
                values: Vec::new(),
                percentile: aggregate_param_f64(args, 1).unwrap_or(50.0),
            }),
            AggregateFunction::PercentileCont => Some(Self::PercentileCont {
                values: Vec::new(),
                percentile: aggregate_param_f64(args, 1).unwrap_or(50.0),
            }),
            AggregateFunction::Median => Some(Self::Median(Vec::new())),
            AggregateFunction::Mode => Some(Self::Mode(Vec::new())),
            AggregateFunction::Std => Some(Self::Std {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::StddevPop => Some(Self::StddevPop {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::StddevSamp => Some(Self::StddevSamp {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::Variance => Some(Self::Variance {
                n: 0,
                mean: 0.0,
                m2: 0.0,
            }),
            AggregateFunction::Product => Some(Self::Product(None)),
            AggregateFunction::BitAnd => Some(Self::BitAnd(None)),
            AggregateFunction::BitOr => Some(Self::BitOr(None)),
            AggregateFunction::BoolAnd => Some(Self::BoolAnd(None)),
            AggregateFunction::BoolOr => Some(Self::BoolOr(None)),
            AggregateFunction::GroupConcat => Some(Self::GroupConcat {
                parts: Vec::new(),
                separator: aggregate_param_string(args, 1).unwrap_or_else(|| ",".to_string()),
            }),
            AggregateFunction::GroupConcatWithOrder => Some(Self::GroupConcatWithOrder {
                parts: Vec::new(),
                separator: aggregate_param_string(args, 1).unwrap_or_else(|| ",".to_string()),
            }),
            AggregateFunction::VecSum => Some(Self::VecSum(None)),
            AggregateFunction::VecAvg => Some(Self::VecAvg {
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
            (Self::CollectSet(a), Self::CollectSet(b)) | (Self::Distinct(a), Self::Distinct(b)) => {
                a.extend(b.iter().cloned())
            }
            (Self::Percentile { values: a, .. }, Self::Percentile { values: b, .. })
            | (Self::PercentileCont { values: a, .. }, Self::PercentileCont { values: b, .. }) => {
                a.extend_from_slice(b)
            }
            (Self::Median(a), Self::Median(b)) => a.extend_from_slice(b),
            (Self::Mode(a), Self::Mode(b)) => a.extend_from_slice(b),
            (
                Self::Std {
                    n: n1,
                    mean: m1,
                    m2: v1,
                },
                Self::Std {
                    n: n2,
                    mean: m2,
                    m2: v2,
                },
            )
            | (
                Self::StddevPop {
                    n: n1,
                    mean: m1,
                    m2: v1,
                },
                Self::StddevPop {
                    n: n2,
                    mean: m2,
                    m2: v2,
                },
            )
            | (
                Self::StddevSamp {
                    n: n1,
                    mean: m1,
                    m2: v1,
                },
                Self::StddevSamp {
                    n: n2,
                    mean: m2,
                    m2: v2,
                },
            )
            | (
                Self::Variance {
                    n: n1,
                    mean: m1,
                    m2: v1,
                },
                Self::Variance {
                    n: n2,
                    mean: m2,
                    m2: v2,
                },
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
            (Self::VecAvg { sum: s1, count: c1 }, Self::VecAvg { sum: s2, count: c2 }) => {
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
                    let mut list = linkrs_core::value::List::new();
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
            | Self::PercentileCont { values, percentile } => percentile_of(values, *percentile),
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

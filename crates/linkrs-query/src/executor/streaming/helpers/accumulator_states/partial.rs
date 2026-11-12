use linkrs_core::types::expr::Expression;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use super::accumulator::AggregateAccumulator;
use super::stats::{f64_list_from_value, value_list_of_f64, value_list_of_values};

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
            let mut list = linkrs_core::value::List::new();
            list.push(Value::Double(*sum));
            list.push(Value::BigInt(*count as i64));
            Value::List(Box::new(list))
        }
        AggregateAccumulator::Collect(values) => value_list_of_values(values),
        AggregateAccumulator::CollectSet(set) | AggregateAccumulator::Distinct(set) => {
            if set.is_empty() {
                Value::Null(NullType::Null)
            } else {
                Value::List(Box::new(linkrs_core::value::List::from(
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
                let mut list = linkrs_core::value::List::new();
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
                Value::List(Box::new(linkrs_core::value::List::from(
                    parts.iter().cloned().map(Value::string).collect::<Vec<_>>(),
                )))
            }
        }
        AggregateAccumulator::VecSum(Some(vec)) => {
            value_list_of_f64(&vec.iter().map(|x| *x as f64).collect::<Vec<_>>())
        }
        AggregateAccumulator::VecSum(None) => Value::Null(NullType::Null),
        AggregateAccumulator::VecAvg { sum, count } => {
            if *count == 0 {
                Value::Null(NullType::Null)
            } else {
                let mut list = linkrs_core::value::List::new();
                let mut inner = linkrs_core::value::List::new();
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
    decode_partial_with_args(func, &[], value)
}

/// Decode a partial-accumulator Value back into an accumulator state,
/// using the given args to initialize parameters (separator, percentile, etc.).
pub fn decode_partial_with_args(
    func: &AggregateFunction,
    args: &[Expression],
    value: &Value,
) -> Option<AggregateAccumulator> {
    let mut acc = AggregateAccumulator::for_function(func, args)?;
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
                *parts = list.values.iter().map(|v| format!("{}", v)).collect();
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

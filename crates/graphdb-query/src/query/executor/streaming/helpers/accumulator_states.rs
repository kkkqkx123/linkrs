use super::comparison::compare_values;
use crate::core::types::operators::AggregateFunction;
use crate::core::value::NullType;
use crate::core::Value;

/// Typed accumulator state for partial aggregate results.
/// Each variant corresponds to an aggregate function that supports
/// partial (per-partition) + final (merge) execution.
#[derive(Debug, Clone)]
pub enum AggregateAccumulator {
    Count(u64),
    Sum(f64),
    Min(Option<Value>),
    Max(Option<Value>),
    Avg { sum: f64, count: u64 },
}

/// Errors that can occur during accumulator operations.
#[derive(Debug)]
pub enum AccumulatorError {
    Overflow,
    TypeMismatch {
        expected: &'static str,
        actual: &'static str,
    },
}

impl AggregateAccumulator {
    pub fn for_function(func: &AggregateFunction) -> Option<Self> {
        match func {
            AggregateFunction::Count(_) => Some(Self::Count(0)),
            AggregateFunction::Sum(_) => Some(Self::Sum(0.0)),
            AggregateFunction::Min(_) => Some(Self::Min(None)),
            AggregateFunction::Max(_) => Some(Self::Max(None)),
            AggregateFunction::Avg(_) => Some(Self::Avg { sum: 0.0, count: 0 }),
            _ => None,
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
            _ => {}
        }
    }

    /// Produce the final aggregate Value from this accumulator.
    pub fn finalize(&self) -> Value {
        match self {
            Self::Count(count) => Value::BigInt(*count as i64),
            Self::Sum(sum) => {
                if sum.fract() == 0.0 && sum.is_finite() {
                    Value::BigInt(*sum as i64)
                } else {
                    Value::Double(*sum)
                }
            }
            Self::Min(Some(v)) | Self::Max(Some(v)) => v.clone(),
            Self::Min(None) | Self::Max(None) => Value::Null(NullType::Null),
            Self::Avg { sum, count } => {
                if *count == 0 {
                    Value::Null(NullType::Null)
                } else {
                    Value::Double(*sum / *count as f64)
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

/// Serialize an accumulator state into a Value for inter-operator transfer.
/// The partial aggregate output stores accumulator states as Value rows
/// where each accumulator is encoded as a list [type_tag, ...fields].
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
    }
}

/// Produce the accumulator final value from an encoded partial Value.
/// This reverses `accumulator_to_value` and calls `finalize`.
pub fn finalize_accumulator_value(
    func: &AggregateFunction,
    partial_value: &Value,
    _count_accumulator: Option<&AggregateAccumulator>,
) -> Value {
    let mut acc = match AggregateAccumulator::for_function(func) {
        Some(a) => a,
        None => return partial_value.clone(),
    };
    match &mut acc {
        AggregateAccumulator::Count(c) => {
            if let Value::BigInt(n) = partial_value {
                *c = *n as u64;
            }
        }
        AggregateAccumulator::Sum(s) => {
            if let Value::Double(n) = partial_value {
                *s = *n;
            } else if let Value::BigInt(n) = partial_value {
                *s = *n as f64;
            }
        }
        AggregateAccumulator::Min(ref mut v) | AggregateAccumulator::Max(ref mut v) => {
            if !matches!(partial_value, Value::Null(_)) {
                let _ = v.insert(partial_value.clone());
            }
        }
        AggregateAccumulator::Avg { sum, count } => {
            if let Value::List(list) = partial_value {
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
    }
    acc.finalize()
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
    fn test_min_max() {
        let mut min_acc = AggregateAccumulator::Min(None);
        let mut max_acc = AggregateAccumulator::Max(None);
        min_acc.accumulate(&Value::Int(5));
        min_acc.accumulate(&Value::Int(3));
        min_acc.accumulate(&Value::Int(7));
        max_acc.accumulate(&Value::Int(5));
        max_acc.accumulate(&Value::Int(3));
        max_acc.accumulate(&Value::Int(7));
        assert_eq!(min_acc.finalize(), Value::Int(3));
        assert_eq!(max_acc.finalize(), Value::Int(7));
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
    fn test_for_function_mapping() {
        assert!(AggregateAccumulator::for_function(&AggregateFunction::Count(None)).is_some());
        assert!(
            AggregateAccumulator::for_function(&AggregateFunction::Sum("x".to_string())).is_some()
        );
        assert!(
            AggregateAccumulator::for_function(&AggregateFunction::Min("x".to_string())).is_some()
        );
        assert!(
            AggregateAccumulator::for_function(&AggregateFunction::Max("x".to_string())).is_some()
        );
        assert!(
            AggregateAccumulator::for_function(&AggregateFunction::Avg("x".to_string())).is_some()
        );
        assert!(
            AggregateAccumulator::for_function(&AggregateFunction::Collect("x".to_string()))
                .is_none()
        );
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
}

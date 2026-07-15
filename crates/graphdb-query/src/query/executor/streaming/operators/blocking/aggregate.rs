use std::collections::HashMap;

use crate::core::types::operators::AggregateFunction;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::streaming::helpers::accumulator_states::AggregateAccumulator;
use crate::query::executor::streaming::spill::SpilledFile;

#[derive(Debug)]
pub struct AggregateState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct GroupByState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct PartialAggregateState {
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct FinalAggregateState {
    pub group_map: HashMap<Vec<Value>, Vec<AggregateAccumulator>>,
    pub col_names: Vec<String>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

pub(crate) fn extract_field_value(row: &[Value], col_names: &[String], func: &AggregateFunction) -> Value {
    match func {
        AggregateFunction::Count(None) => Value::Int(1),
        AggregateFunction::Count(Some(field))
        | AggregateFunction::Sum(field)
        | AggregateFunction::Avg(field)
        | AggregateFunction::Min(field)
        | AggregateFunction::Max(field) => {
            if let Some(idx) = col_names.iter().position(|c| c == field) {
                row.get(idx).cloned().unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

pub(crate) fn value_to_partial_accumulator(
    func: &AggregateFunction,
    value: &Value,
) -> Option<AggregateAccumulator> {
    match func {
        AggregateFunction::Count(_) => {
            if let Value::BigInt(n) = value {
                Some(AggregateAccumulator::Count(*n as u64))
            } else {
                Some(AggregateAccumulator::Count(0))
            }
        }
        AggregateFunction::Sum(_) => match value {
            Value::Double(n) => Some(AggregateAccumulator::Sum(*n)),
            Value::BigInt(n) => Some(AggregateAccumulator::Sum(*n as f64)),
            Value::Int(n) => Some(AggregateAccumulator::Sum(*n as f64)),
            _ => Some(AggregateAccumulator::Sum(0.0)),
        },
        AggregateFunction::Min(_) => {
            if matches!(value, Value::Null(_)) {
                Some(AggregateAccumulator::Min(None))
            } else {
                Some(AggregateAccumulator::Min(Some(value.clone())))
            }
        }
        AggregateFunction::Max(_) => {
            if matches!(value, Value::Null(_)) {
                Some(AggregateAccumulator::Max(None))
            } else {
                Some(AggregateAccumulator::Max(Some(value.clone())))
            }
        }
        AggregateFunction::Avg(_) => {
            if let Value::List(list) = value {
                let sum = list
                    .values
                    .first()
                    .and_then(|v| match v {
                        Value::Double(n) => Some(*n),
                        Value::BigInt(n) => Some(*n as f64),
                        _ => None,
                    })
                    .unwrap_or(0.0);
                let count = list
                    .values
                    .get(1)
                    .and_then(|v| match v {
                        Value::BigInt(n) => Some(*n as u64),
                        _ => None,
                    })
                    .unwrap_or(0);
                Some(AggregateAccumulator::Avg { sum, count })
            } else {
                Some(AggregateAccumulator::Avg { sum: 0.0, count: 0 })
            }
        }
        _ => None,
    }
}

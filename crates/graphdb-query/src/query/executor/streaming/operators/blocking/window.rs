use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::streaming::spill::SpilledFile;

#[derive(Debug)]
pub struct WindowFunctionState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

#[derive(Debug)]
pub struct WindowState {
    pub all_rows: Vec<Vec<Value>>,
    pub result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    pub spill_files: Vec<SpilledFile>,
}

pub(crate) fn compute_window_function(
    name: &str,
    args: &[Value],
    partition_rows: &[(usize, Vec<Value>)],
    current_pos: usize,
) -> Value {
    let value_to_i64 = |val: &Value| -> i64 {
        match val {
            Value::Int(i) => *i as i64,
            Value::BigInt(i) => *i,
            _ => 1,
        }
    };

    match name {
        "row_number" => Value::BigInt((current_pos + 1) as i64),
        "rank" => Value::BigInt((current_pos + 1) as i64),
        "dense_rank" => Value::BigInt((current_pos + 1) as i64),
        "lead" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos + offset < partition_rows.len() {
                partition_rows[current_pos + offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "lag" => {
            let offset = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            let default_val = if args.len() > 1 {
                args[1].clone()
            } else {
                Value::Null(NullType::Null)
            };
            if current_pos >= offset {
                partition_rows[current_pos - offset]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(default_val)
            } else {
                default_val
            }
        }
        "first_value" => partition_rows
            .first()
            .and_then(|(_, r)| r.first().cloned())
            .unwrap_or(Value::Null(NullType::Null)),
        "last_value" => partition_rows
            .last()
            .and_then(|(_, r)| r.first().cloned())
            .unwrap_or(Value::Null(NullType::Null)),
        "nth_value" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0]) as usize
            } else {
                1
            };
            if n > 0 && n <= partition_rows.len() {
                partition_rows[n - 1]
                    .1
                    .first()
                    .cloned()
                    .unwrap_or(Value::Null(NullType::Null))
            } else {
                Value::Null(NullType::Null)
            }
        }
        "ntile" => {
            let n = if !args.is_empty() {
                value_to_i64(&args[0])
            } else {
                1
            };
            if n > 0 {
                let bucket_size = (partition_rows.len() as i64 + n - 1) / n;
                Value::BigInt((current_pos as i64 / bucket_size) + 1)
            } else {
                Value::Null(NullType::Null)
            }
        }
        _ => Value::Null(NullType::Null),
    }
}

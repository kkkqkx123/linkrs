//! Table function definitions.
//!
//! Table functions return rows of data instead of a single scalar value.

use crate::executor::expression::{ExpressionError, ExpressionErrorType};
use linkrs_core::Value;

/// Table function trait for functions that return rows of data.
///
/// Unlike scalar functions that return a single `Value`, table functions
/// return multiple rows, each row being a `Vec<Value>`.
pub trait TableFunction: Send + Sync + std::fmt::Debug {
    /// Function name
    fn name(&self) -> &str;

    /// Execute the table function with the given arguments.
    /// Returns a vector of rows, where each row is a vector of values.
    fn execute(&self, args: &[Value]) -> Result<Vec<Vec<Value>>, ExpressionError>;

    /// Function description
    fn description(&self) -> &str;
}

/// Built-in table function implementations.
#[derive(Debug, Clone)]
pub enum BuiltinTableFunction {
    /// Read CSV files
    ReadCsv,
}

impl TableFunction for BuiltinTableFunction {
    fn name(&self) -> &str {
        match self {
            BuiltinTableFunction::ReadCsv => "read_csv",
        }
    }

    fn execute(&self, args: &[Value]) -> Result<Vec<Vec<Value>>, ExpressionError> {
        match self {
            BuiltinTableFunction::ReadCsv => execute_read_csv(args),
        }
    }

    fn description(&self) -> &str {
        match self {
            BuiltinTableFunction::ReadCsv => "Read a CSV file and return its rows",
        }
    }
}

fn execute_read_csv(args: &[Value]) -> Result<Vec<Vec<Value>>, ExpressionError> {
    if args.is_empty() {
        return Err(ExpressionError::new(
            ExpressionErrorType::InvalidArgumentCount,
            "read_csv expects at least 1 argument (path)".to_string(),
        ));
    }

    let path = match &args[0] {
        Value::String(s) => s.clone(),
        other => {
            return Err(ExpressionError::new(
                ExpressionErrorType::TypeError,
                format!("read_csv path must be a string, got {:?}", other),
            ))
        }
    };

    let delimiter = if args.len() > 1 {
        match &args[1] {
            Value::String(s) => s.as_bytes().first().copied().unwrap_or(b','),
            _ => b',',
        }
    } else {
        b','
    };

    let file = std::fs::File::open(&path).map_err(|e| {
        ExpressionError::new(
            ExpressionErrorType::FunctionExecutionError,
            format!("read_csv failed to open '{path}': {e}"),
        )
    })?;

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .delimiter(delimiter)
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(std::io::BufReader::new(file));

    let mut rows: Vec<Vec<Value>> = Vec::new();
    for result in reader.records() {
        let record = result.map_err(|e| {
            ExpressionError::new(
                ExpressionErrorType::FunctionExecutionError,
                format!("read_csv row error: {e}"),
            )
        })?;
        let row: Vec<Value> = record.iter().map(|s| Value::String(s.into())).collect();
        rows.push(row);
    }

    Ok(rows)
}

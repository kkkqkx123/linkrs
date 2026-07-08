//! Module for defining built-in function macros
//!
//! Provide macros for reducing the amount of样板 code, used for defining function enumerations and for executing functions.

/// Macro for defining an enumeration of built-in functions
///
/// 自动生成 name(), arity(), is_variadic(), description(), execute() 方法
#[macro_export]
macro_rules! define_function_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => {
                    name: $func_name:literal,
                    arity: $arity:expr,
                    variadic: $variadic:expr,
                    description: $desc:literal,
                    handler: $handler:expr
                }
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )*
        }

        impl $name {
            /// Obtain the function name
            $vis fn name(&self) -> &str {
                match self {
                    $(Self::$variant => $func_name,)*
                }
            }

            /// Determine the number of parameters
            $vis fn arity(&self) -> usize {
                match self {
                    $(Self::$variant => $arity,)*
                }
            }

            /// Is it a function with variable parameters?
            $vis fn is_variadic(&self) -> bool {
                match self {
                    $(Self::$variant => $variadic,)*
                }
            }

            /// Obtain the function description
            $vis fn description(&self) -> &str {
                match self {
                    $(Self::$variant => $desc,)*
                }
            }

            /// Execute the function
            $vis fn execute(&self, args: &[$crate::core::Value]) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
                let handler: fn(&[$crate::core::Value]) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> = match self {
                    $(Self::$variant => $handler,)*
                };
                handler(args)
            }
        }
    };
}

/// Define a single-parameter numerical function that returns a Float value.
#[macro_export]
macro_rules! define_unary_float_fn {
    ($name:ident, $op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.is_empty() {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 1 argument"),
                ));
            }

            let op = $op;
            match &args[0] {
                Value::SmallInt(i) => Ok(Value::Float(op(*i as f32))),
                Value::Int(i) => Ok(Value::Float(op(*i as f32))),
                Value::BigInt(i) => Ok(Value::Double(op(*i as f32) as f64)),
                Value::Float(f) => Ok(Value::Float(op(*f))),
                Value::Double(f) => Ok(Value::Double(op(*f as f32) as f64)),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require numeric types"
                    )),
                ),
            }
        }
    };
}

/// Define a single-parameter integer/float function (while preserving the data type)
#[macro_export]
macro_rules! define_unary_numeric_fn {
    ($name:ident, int: $int_op:expr, float: $float_op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.is_empty() {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 1 argument"),
                ));
            }

            match &args[0] {
                Value::SmallInt(i) => $int_op(*i as i32),
                Value::Int(i) => $int_op(*i),
                Value::BigInt(i) => $int_op(*i as i32),
                Value::Float(f) => $float_op(*f),
                Value::Double(f) => $float_op(*f as f32),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require numeric types"
                    )),
                ),
            }
        }
    };
}

/// Define a single-parameter string function
#[macro_export]
macro_rules! define_unary_string_fn {
    ($name:ident, $op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.is_empty() {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 1 argument"),
                ));
            }

            let op = $op;
            match &args[0] {
                Value::String(s) => Ok(Value::String(op(s))),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require a string type"
                    )),
                ),
            }
        }
    };
}

/// Define a function for extracting date and time fields
#[macro_export]
macro_rules! define_datetime_extractor {
    ($name:ident, Date => $date_field:ident, DateTime => $datetime_field:ident) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.is_empty() {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!(stringify!($name), "The function takes 1 argument"),
                ));
            }

            match &args[0] {
                Value::Date(d) => Ok(Value::BigInt(d.$date_field as i64)),
                Value::DateTime(dt) => Ok(Value::BigInt(dt.$datetime_field as i64)),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        stringify!($name),
                        "Functions require a date or datetime type"
                    )),
                ),
            }
        }
    };
    ($name:ident, Time => $time_field:ident, DateTime => $datetime_field:ident) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.is_empty() {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!(stringify!($name), "The function takes 1 argument"),
                ));
            }

            match &args[0] {
                Value::Time(t) => Ok(Value::BigInt(t.$time_field as i64)),
                Value::DateTime(dt) => Ok(Value::BigInt(dt.$datetime_field as i64)),
                Value::Null(_) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        stringify!($name),
                        "Functions require a time or datetime type"
                    )),
                ),
            }
        }
    };
}

/// Define a wrapper function that performs a check on the number of parameters
#[macro_export]
macro_rules! define_arg_checked_fn {
    ($name:ident, $arity:expr, $handler:expr, $type_desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.len() != $arity {
                return Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        stringify!($name),
                        "The function requires",
                        stringify!($arity),
                        "specifications"
                    )),
                );
            }
            $handler(args)
        }
    };
}

/// Define a binary numeric operation function
#[macro_export]
macro_rules! define_binary_numeric_fn {
    ($name:ident, $op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.len() != 2 {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 2 arguments"),
                ));
            }

            let op = $op;
            match (&args[0], &args[1]) {
                (Value::SmallInt(a), Value::SmallInt(b)) => op(*a as f32, *b as f32),
                (Value::Int(a), Value::Int(b)) => op(*a as f32, *b as f32),
                (Value::BigInt(a), Value::BigInt(b)) => op(*a as f32, *b as f32),
                (Value::SmallInt(a), Value::Int(b)) => op(*a as f32, *b as f32),
                (Value::Int(a), Value::SmallInt(b)) => op(*a as f32, *b as f32),
                (Value::Float(a), Value::Float(b)) => op(*a, *b),
                (Value::Double(a), Value::Double(b)) => op(*a as f32, *b as f32),
                (Value::Float(a), Value::Double(b)) => op(*a, *b as f32),
                (Value::Double(a), Value::Float(b)) => op(*a as f32, *b),
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require numeric types"
                    )),
                ),
            }
        }
    };
}

/// Define a binary string comparison function
#[macro_export]
macro_rules! define_binary_string_bool_fn {
    ($name:ident, $op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.len() != 2 {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 2 arguments"),
                ));
            }

            let op = $op;
            match (&args[0], &args[1]) {
                (Value::String(a), Value::String(b)) => Ok(Value::Bool(op(a, b))),
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require a string type"
                    )),
                ),
            }
        }
    };
}

/// Define a geospatial binary function
#[macro_export]
macro_rules! define_binary_geography_fn {
    ($name:ident, $op:expr, $desc:literal) => {
        fn $name(
            args: &[$crate::core::Value],
        ) -> Result<$crate::core::Value, $crate::query::executor::expression::ExpressionError> {
            if args.len() != 2 {
                return Err($crate::query::executor::expression::ExpressionError::new(
                    $crate::query::executor::expression::ExpressionErrorType::InvalidArgumentCount,
                    concat!($desc, "The function takes 2 arguments"),
                ));
            }

            let op = $op;
            match (&args[0], &args[1]) {
                (Value::Geography(geo1), Value::Geography(geo2)) => op(geo1, geo2),
                (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
                _ => Err(
                    $crate::query::executor::expression::ExpressionError::type_error(concat!(
                        $desc,
                        "Functions require geo-typed parameters"
                    )),
                ),
            }
        }
    };
}

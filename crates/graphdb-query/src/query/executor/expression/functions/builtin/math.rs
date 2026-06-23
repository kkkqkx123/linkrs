//! Implementation of mathematical functions

use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

define_function_enum! {
    /// Enumeration of mathematical functions
    pub enum MathFunction {
        Abs => {
            name: "abs",
            arity: 1,
            variadic: false,
            description: "Calculating Absolute Values",
            handler: execute_abs
        },
        Sqrt => {
            name: "sqrt",
            arity: 1,
            variadic: false,
            description: "square root calculation",
            handler: execute_sqrt
        },
        Pow => {
            name: "pow",
            arity: 2,
            variadic: false,
            description: "exponentiate (math.)",
            handler: execute_pow
        },
        Log => {
            name: "log",
            arity: 2,
            variadic: false,
            description: "logarithmic",
            handler: execute_log
        },
        Log10 => {
            name: "log10",
            arity: 1,
            variadic: false,
            description: "Calculating logarithms with base 10",
            handler: execute_log10
        },
        Sin => {
            name: "sin",
            arity: 1,
            variadic: false,
            description: "calculate the sine",
            handler: execute_sin
        },
        Cos => {
            name: "cos",
            arity: 1,
            variadic: false,
            description: "calculate the cosine",
            handler: execute_cos
        },
        Tan => {
            name: "tan",
            arity: 1,
            variadic: false,
            description: "arithmetic tangent (math.)",
            handler: execute_tan
        },
        Round => {
            name: "round",
            arity: 1,
            variadic: false,
            description: "discard four, but treat five as whole (of decimal points)",
            handler: execute_round
        },
        Ceil => {
            name: "ceil",
            arity: 1,
            variadic: false,
            description: "Round up",
            handler: execute_ceil
        },
        Floor => {
            name: "floor",
            arity: 1,
            variadic: false,
            description: "round down",
            handler: execute_floor
        },
        Asin => {
            name: "asin",
            arity: 1,
            variadic: false,
            description: "calculate the arcsine",
            handler: execute_asin
        },
        Acos => {
            name: "acos",
            arity: 1,
            variadic: false,
            description: "Calculate the inverse cosine",
            handler: execute_acos
        },
        Atan => {
            name: "atan",
            arity: 1,
            variadic: false,
            description: "Compute the arctangent",
            handler: execute_atan
        },
        Cbrt => {
            name: "cbrt",
            arity: 1,
            variadic: false,
            description: "calculate the cube root",
            handler: execute_cbrt
        },
        Hypot => {
            name: "hypot",
            arity: 2,
            variadic: false,
            description: "Compute the hypotenuse of a right triangle",
            handler: execute_hypot
        },
        Sign => {
            name: "sign",
            arity: 1,
            variadic: false,
            description: "Return Value Symbol",
            handler: execute_sign
        },
        Rand => {
            name: "rand",
            arity: 0,
            variadic: false,
            description: "Generate random floating point numbers",
            handler: execute_rand
        },
        Rand32 => {
            name: "rand32",
            arity: 0,
            variadic: true,
            description: "Generate 32-bit random integers",
            handler: execute_rand32
        },
        Rand64 => {
            name: "rand64",
            arity: 0,
            variadic: false,
            description: "Generate 64-bit random integers",
            handler: execute_rand64
        },
        E => {
            name: "e",
            arity: 0,
            variadic: false,
            description: "Returns the natural constant e",
            handler: execute_e
        },
        Pi => {
            name: "pi",
            arity: 0,
            variadic: false,
            description: "Return to pi",
            handler: execute_pi
        },
        Exp2 => {
            name: "exp2",
            arity: 1,
            variadic: false,
            description: "Calculating powers of 2",
            handler: execute_exp2
        },
        Log2 => {
            name: "log2",
            arity: 1,
            variadic: false,
            description: "Calculating logarithms with a base of 2",
            handler: execute_log2
        },
        Radians => {
            name: "radians",
            arity: 1,
            variadic: false,
            description: "Angle to radian",
            handler: execute_radians
        },
        BitAnd => {
            name: "bit_and",
            arity: 2,
            variadic: false,
            description: "compatibility with",
            handler: execute_bit_and
        },
        BitOr => {
            name: "bit_or",
            arity: 2,
            variadic: false,
            description: "push-button or",
            handler: execute_bit_or
        },
        BitXor => {
            name: "bit_xor",
            arity: 2,
            variadic: false,
            description: "palindromic or binomial (math.)",
            handler: execute_bit_xor
        },
    }
}

define_unary_numeric_fn!(
    execute_abs,
    int: |i: i32| Ok(Value::Int(i.abs())),
    float: |f: f32| Ok(Value::Float(f.abs())),
    "abs"
);

define_unary_float_fn!(execute_sqrt, |v: f32| v.sqrt(), "sqrt");
define_unary_float_fn!(execute_sin, |v: f32| v.sin(), "sin");
define_unary_float_fn!(execute_cos, |v: f32| v.cos(), "cos");
define_unary_float_fn!(execute_tan, |v: f32| v.tan(), "tan");
define_unary_float_fn!(execute_log10, |v: f32| v.log10(), "log10");

define_unary_numeric_fn!(
    execute_round,
    int: |i: i32| Ok(Value::Int(i)),
    float: |f: f32| Ok(Value::Float(f.round())),
    "round"
);

define_unary_numeric_fn!(
    execute_ceil,
    int: |i: i32| Ok(Value::Float(i as f32)),
    float: |f: f32| Ok(Value::Float(f.ceil())),
    "ceil"
);

define_unary_numeric_fn!(
    execute_floor,
    int: |i: i32| Ok(Value::Float(i as f32)),
    float: |f: f32| Ok(Value::Float(f.floor())),
    "floor"
);

define_binary_numeric_fn!(
    execute_pow,
    |a: f32, b: f32| Ok(Value::Float(a.powf(b))),
    "pow"
);

define_binary_numeric_fn!(
    execute_log,
    |base: f32, val: f32| Ok(Value::Float(val.log(base))),
    "log"
);

// New implementation of mathematical functions
define_unary_float_fn!(execute_asin, |v: f32| v.asin(), "asin");
define_unary_float_fn!(execute_acos, |v: f32| v.acos(), "acos");
define_unary_float_fn!(execute_atan, |v: f32| v.atan(), "atan");
define_unary_float_fn!(execute_cbrt, |v: f32| v.cbrt(), "cbrt");
define_unary_float_fn!(execute_exp2, |v: f32| v.exp2(), "exp2");
define_unary_float_fn!(execute_log2, |v: f32| v.log2(), "log2");
define_unary_float_fn!(execute_radians, |v: f32| v.to_radians(), "radians");

define_binary_numeric_fn!(
    execute_hypot,
    |a: f32, b: f32| Ok(Value::Float(a.hypot(b))),
    "hypot"
);

fn execute_sign(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "The sign function takes 1 argument",
        ));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::SmallInt(i.signum())),
        Value::Int(i) => Ok(Value::Int(i.signum())),
        Value::BigInt(i) => Ok(Value::BigInt(i.signum())),
        Value::Float(f) => Ok(Value::Int(f.signum() as i32)),
        Value::Double(f) => Ok(Value::BigInt(f.signum() as i64)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The sign function requires a numeric type",
        )),
    }
}

fn execute_rand(_args: &[Value]) -> Result<Value, ExpressionError> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    Ok(Value::Double(rng.gen::<f64>()))
}

fn execute_rand32(args: &[Value]) -> Result<Value, ExpressionError> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let result = match args.len() {
        0 => rng.gen::<i32>(),
        1 => match &args[0] {
            Value::Int(max) => rng.gen_range(0..*max),
            Value::Null(_) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(
                    "The rand32 function takes integer arguments",
                ))
            }
        },
        2 => match (&args[0], &args[1]) {
            (Value::Int(min), Value::Int(max)) => rng.gen_range(*min..*max),
            (Value::Null(_), _) | (_, Value::Null(_)) => return Ok(Value::Null(NullType::Null)),
            _ => {
                return Err(ExpressionError::type_error(
                    "The rand32 function takes integer arguments",
                ))
            }
        },
        _ => {
            return Err(ExpressionError::type_error(
                "The rand32 function takes 0-2 arguments",
            ))
        }
    };
    Ok(Value::Int(result))
}

fn execute_rand64(_args: &[Value]) -> Result<Value, ExpressionError> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    Ok(Value::BigInt(rng.gen::<i64>()))
}

fn execute_e(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::Double(std::f64::consts::E))
}

fn execute_pi(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::Double(std::f64::consts::PI))
}

fn execute_bit_and(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The bit_and function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::SmallInt(a), Value::SmallInt(b)) => Ok(Value::SmallInt(a & b)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a & b)),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(a & b)),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The bit_and function takes integer arguments",
        )),
    }
}

fn execute_bit_or(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The bit_or function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::SmallInt(a), Value::SmallInt(b)) => Ok(Value::SmallInt(a | b)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a | b)),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(a | b)),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The bit_or function takes integer arguments",
        )),
    }
}

fn execute_bit_xor(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The bit_xor function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::SmallInt(a), Value::SmallInt(b)) => Ok(Value::SmallInt(a ^ b)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a ^ b)),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(a ^ b)),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The bit_xor function takes integer arguments",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abs_int() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Int(-5)])
            .expect("Abs Function Failure");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_abs_float() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Float(-5.5)])
            .expect("Abs Function Failure");
        assert_eq!(result, Value::Float(5.5));
    }

    #[test]
    fn test_sqrt() {
        let func = MathFunction::Sqrt;
        let result = func
            .execute(&[Value::Int(16)])
            .expect("Sqrt function failed to execute");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_pow() {
        let func = MathFunction::Pow;
        let result = func
            .execute(&[Value::Int(2), Value::Int(3)])
            .expect("Pow Function Execution Failure");
        assert_eq!(result, Value::Float(8.0));
    }

    #[test]
    fn test_sin() {
        let func = MathFunction::Sin;
        let result = func
            .execute(&[Value::Float(0.0)])
            .expect("Sin Function Failure");
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn test_cos() {
        let func = MathFunction::Cos;
        let result = func
            .execute(&[Value::Float(0.0)])
            .expect("Cos Function Failure");
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn test_round() {
        let func = MathFunction::Round;
        let result = func
            .execute(&[Value::Float(3.7)])
            .expect("Round Function Failure");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_ceil() {
        let func = MathFunction::Ceil;
        let result = func
            .execute(&[Value::Float(3.2)])
            .expect("Ceil Function Execution Failure");
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_floor() {
        let func = MathFunction::Floor;
        let result = func
            .execute(&[Value::Float(3.9)])
            .expect("Floor function failed to execute");
        assert_eq!(result, Value::Float(3.0));
    }

    #[test]
    fn test_null_handling() {
        let func = MathFunction::Abs;
        let result = func
            .execute(&[Value::Null(NullType::Null)])
            .expect("Abs function null handling failure");
        assert_eq!(result, Value::Null(NullType::Null));
    }
}

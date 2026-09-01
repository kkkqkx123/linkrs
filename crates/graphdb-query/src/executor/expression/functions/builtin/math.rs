//! Implementation of mathematical functions

use crate::executor::expression::ExpressionError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

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
        Atan2 => {
            name: "atan2",
            arity: 2,
            variadic: false,
            description: "Compute the arctangent of y/x",
            handler: execute_atan2
        },
        Sinh => {
            name: "sinh",
            arity: 1,
            variadic: false,
            description: "Calculate the hyperbolic sine",
            handler: execute_sinh
        },
        Cosh => {
            name: "cosh",
            arity: 1,
            variadic: false,
            description: "Calculate the hyperbolic cosine",
            handler: execute_cosh
        },
        Tanh => {
            name: "tanh",
            arity: 1,
            variadic: false,
            description: "Calculate the hyperbolic tangent",
            handler: execute_tanh
        },
        Degrees => {
            name: "degrees",
            arity: 1,
            variadic: false,
            description: "Convert radians to degrees",
            handler: execute_degrees
        },
        Gcd => {
            name: "gcd",
            arity: 2,
            variadic: false,
            description: "Greatest common divisor",
            handler: execute_gcd
        },
        Lcm => {
            name: "lcm",
            arity: 2,
            variadic: false,
            description: "Least common multiple",
            handler: execute_lcm
        },
        Factorial => {
            name: "factorial",
            arity: 1,
            variadic: false,
            description: "Calculate factorial of a number",
            handler: execute_factorial
        },
        Gamma => {
            name: "gamma",
            arity: 1,
            variadic: false,
            description: "Calculate gamma function",
            handler: execute_gamma
        },
        Lgamma => {
            name: "lgamma",
            arity: 1,
            variadic: false,
            description: "Calculate natural logarithm of absolute value of gamma function",
            handler: execute_lgamma
        },
        Negate => {
            name: "negate",
            arity: 1,
            variadic: false,
            description: "Negate a number",
            handler: execute_negate
        },
        Even => {
            name: "even",
            arity: 1,
            variadic: false,
            description: "Round up to nearest even integer",
            handler: execute_even
        },
        SetSeed => {
            name: "set_seed",
            arity: 1,
            variadic: false,
            description: "Set random seed",
            handler: execute_set_seed
        },
        BitShiftLeft => {
            name: "bit_shift_left",
            arity: 2,
            variadic: false,
            description: "Bitwise left shift",
            handler: execute_bit_shift_left
        },
        BitShiftRight => {
            name: "bit_shift_right",
            arity: 2,
            variadic: false,
            description: "Bitwise right shift",
            handler: execute_bit_shift_right
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
    Ok(Value::Float(rng.gen::<f32>()))
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
    Ok(Value::Float(std::f32::consts::E))
}

fn execute_pi(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::Float(std::f32::consts::PI))
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

define_binary_numeric_fn!(
    execute_atan2,
    |y: f32, x: f32| Ok(Value::Float(y.atan2(x))),
    "atan2"
);

define_unary_float_fn!(execute_sinh, |v: f32| v.sinh(), "sinh");
define_unary_float_fn!(execute_cosh, |v: f32| v.cosh(), "cosh");
define_unary_float_fn!(execute_tanh, |v: f32| v.tanh(), "tanh");
define_unary_float_fn!(execute_degrees, |v: f32| v.to_degrees(), "degrees");

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

fn execute_gcd(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The gcd function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(gcd(*a, *b))),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(gcd(*a, *b))),
        (Value::Int(a), Value::BigInt(b)) => Ok(Value::BigInt(gcd(*a as i64, *b))),
        (Value::BigInt(a), Value::Int(b)) => Ok(Value::BigInt(gcd(*a, *b as i64))),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The gcd function requires integer arguments",
        )),
    }
}

fn execute_lcm(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "The lcm function takes 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(lcm(*a, *b))),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(lcm(*a, *b))),
        (Value::Int(a), Value::BigInt(b)) => Ok(Value::BigInt(lcm(*a as i64, *b))),
        (Value::BigInt(a), Value::Int(b)) => Ok(Value::BigInt(lcm(*a, *b as i64))),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The lcm function requires integer arguments",
        )),
    }
}

fn gcd<
    T: Copy
        + std::ops::Rem<Output = T>
        + PartialEq
        + From<i8>
        + std::ops::Neg<Output = T>
        + PartialOrd,
>(
    mut a: T,
    mut b: T,
) -> T {
    let zero: T = T::from(0);
    if a == zero {
        return b;
    }
    if b == zero {
        return a;
    }
    while b != zero {
        let temp = b;
        b = a % b;
        a = temp;
    }
    if a < zero {
        -a
    } else {
        a
    }
}

fn lcm<
    T: Copy
        + std::ops::Rem<Output = T>
        + PartialEq
        + From<i8>
        + std::ops::Neg<Output = T>
        + PartialOrd
        + std::ops::Div<Output = T>
        + std::ops::Mul<Output = T>,
>(
    a: T,
    b: T,
) -> T {
    let zero: T = T::from(0);
    if a == zero || b == zero {
        return zero;
    }
    let g = gcd(a, b);
    let abs_a = if a < zero { -a } else { a };
    let abs_b = if b < zero { -b } else { b };
    (abs_a / g) * abs_b
}

fn execute_factorial(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error(
            "factorial requires 1 argument",
        ));
    }
    match &args[0] {
        Value::SmallInt(i) => {
            if *i < 0 {
                return Err(ExpressionError::type_error(
                    "factorial requires a non-negative integer",
                ));
            }
            let mut result: i64 = 1;
            for j in 2..=*i as i64 {
                result *= j;
            }
            Ok(Value::BigInt(result))
        }
        Value::Int(i) => {
            if *i < 0 {
                return Err(ExpressionError::type_error(
                    "factorial requires a non-negative integer",
                ));
            }
            let mut result: i64 = 1;
            for j in 2..=*i as i64 {
                result *= j;
            }
            Ok(Value::BigInt(result))
        }
        Value::BigInt(i) => {
            if *i < 0 {
                return Err(ExpressionError::type_error(
                    "factorial requires a non-negative integer",
                ));
            }
            let mut result: i64 = 1;
            for j in 2..=*i {
                result *= j;
            }
            Ok(Value::BigInt(result))
        }
        Value::Float(f) => {
            if *f < 0.0 {
                return Err(ExpressionError::type_error(
                    "factorial requires a non-negative integer",
                ));
            }
            Ok(Value::Float(gamma(*f + 1.0)))
        }
        Value::Double(f) => {
            if *f < 0.0 {
                return Err(ExpressionError::type_error(
                    "factorial requires a non-negative integer",
                ));
            }
            Ok(Value::Float(gamma(*f as f32 + 1.0)))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "factorial requires a numeric type",
        )),
    }
}

fn gamma(mut x: f32) -> f32 {
    if x < 0.5 {
        std::f32::consts::PI / ((std::f32::consts::PI * x).sin() * gamma(1.0 - x))
    } else {
        x -= 1.0;
        let g = 7;
        let c = [
            0.99999999999980993,
            676.5203681218851,
            -1259.1392167224028,
            771.32342877765313,
            -176.61502916214059,
            12.507343278686905,
            -0.13857109526572012,
            9.9843695780195716e-6,
            1.5056327351493116e-7,
        ];
        let mut t = c[0];
        for i in 1..g + 2 {
            t += c[i] / (x + i as f32);
        }
        let x = x + g as f32 + 0.5;
        (2.0 * std::f32::consts::PI).sqrt() * x.powf(x - 0.5) * (-x).exp() * t
    }
}

fn execute_gamma(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("gamma requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::Float(gamma(*i as f32))),
        Value::Int(i) => Ok(Value::Float(gamma(*i as f32))),
        Value::BigInt(i) => Ok(Value::Float(gamma(*i as f32))),
        Value::Float(f) => Ok(Value::Float(gamma(*f))),
        Value::Double(f) => Ok(Value::Float(gamma(*f as f32))),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("gamma requires a numeric type")),
    }
}

fn execute_lgamma(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("lgamma requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::Float(gamma(*i as f32).abs().ln())),
        Value::Int(i) => Ok(Value::Float(gamma(*i as f32).abs().ln())),
        Value::BigInt(i) => Ok(Value::Float(gamma(*i as f32).abs().ln())),
        Value::Float(f) => Ok(Value::Float(gamma(*f).abs().ln())),
        Value::Double(f) => Ok(Value::Float(gamma(*f as f32).abs().ln())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "lgamma requires a numeric type",
        )),
    }
}

fn execute_negate(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("negate requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::SmallInt(-i)),
        Value::Int(i) => Ok(Value::Int(-i)),
        Value::BigInt(i) => Ok(Value::BigInt(-i)),
        Value::Float(f) => Ok(Value::Float(-f)),
        Value::Double(f) => Ok(Value::Double(-f)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("negate requires a numeric type")),
    }
}

fn execute_even(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("even requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(i) => Ok(Value::SmallInt(if *i % 2 == 0 { *i } else { *i + 1 })),
        Value::Int(i) => Ok(Value::Int(if *i % 2 == 0 { *i } else { *i + 1 })),
        Value::BigInt(i) => Ok(Value::BigInt(if *i % 2 == 0 { *i } else { *i + 1 })),
        Value::Float(f) => {
            let rounded = f.round();
            if rounded % 2.0 == 0.0 {
                Ok(Value::Float(rounded))
            } else {
                Ok(Value::Float(rounded + 1.0))
            }
        }
        Value::Double(f) => {
            let rounded = f.round();
            if rounded % 2.0 == 0.0 {
                Ok(Value::Double(rounded))
            } else {
                Ok(Value::Double(rounded + 1.0))
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("even requires a numeric type")),
    }
}

fn execute_set_seed(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 1 {
        return Err(ExpressionError::type_error("set_seed requires 1 argument"));
    }
    match &args[0] {
        Value::SmallInt(_) => Ok(Value::Null(NullType::Null)),
        Value::Int(_) => Ok(Value::Null(NullType::Null)),
        Value::BigInt(_) => Ok(Value::Null(NullType::Null)),
        Value::Float(_) => Ok(Value::Null(NullType::Null)),
        Value::Double(_) => Ok(Value::Null(NullType::Null)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "set_seed requires a numeric type",
        )),
    }
}

fn execute_bit_shift_left(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "bit_shift_left requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::SmallInt(a), Value::SmallInt(b)) => Ok(Value::SmallInt(a << *b)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a << *b)),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(a << *b)),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "bit_shift_left requires integer arguments",
        )),
    }
}

fn execute_bit_shift_right(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "bit_shift_right requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::SmallInt(a), Value::SmallInt(b)) => Ok(Value::SmallInt(a >> *b)),
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a >> *b)),
        (Value::BigInt(a), Value::BigInt(b)) => Ok(Value::BigInt(a >> *b)),
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "bit_shift_right requires integer arguments",
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

    #[test]
    fn test_factorial() {
        let func = MathFunction::Factorial;
        let result = func
            .execute(&[Value::Int(5)])
            .expect("factorial should succeed");
        assert_eq!(result, Value::BigInt(120));
    }

    #[test]
    fn test_gamma() {
        let func = MathFunction::Gamma;
        let result = func
            .execute(&[Value::Int(1)])
            .expect("gamma should succeed");
        assert!(matches!(result, Value::Float(_)));
    }

    #[test]
    fn test_negate() {
        let func = MathFunction::Negate;
        let result = func
            .execute(&[Value::Int(5)])
            .expect("negate should succeed");
        assert_eq!(result, Value::Int(-5));
    }

    #[test]
    fn test_even() {
        let func = MathFunction::Even;
        let result = func
            .execute(&[Value::Int(3)])
            .expect("even should succeed");
        assert_eq!(result, Value::Int(4));
    }

    #[test]
    fn test_bit_shift_left() {
        let func = MathFunction::BitShiftLeft;
        let result = func
            .execute(&[Value::Int(1), Value::Int(3)])
            .expect("bit_shift_left should succeed");
        assert_eq!(result, Value::Int(8));
    }

    #[test]
    fn test_bit_shift_right() {
        let func = MathFunction::BitShiftRight;
        let result = func
            .execute(&[Value::Int(8), Value::Int(2)])
            .expect("bit_shift_right should succeed");
        assert_eq!(result, Value::Int(2));
    }
}

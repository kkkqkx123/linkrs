//! Value Calculation Module
//!
//! This module provides methods for arithmetic, logical, and bitwise operations on values.
use crate::core::value::Value;

impl Value {
    /// Addition operation
    pub fn add(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            // Same type operations
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a.wrapping_add(*b))),
            (Int(a), Int(b)) => Ok(Int(a.wrapping_add(*b))),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a.wrapping_add(*b))),
            (Float(a), Float(b)) => Ok(Float(a + b)),
            (Double(a), Double(b)) => Ok(Double(a + b)),

            // Cross-type operations: promote to larger type
            (SmallInt(a), Int(b)) => Ok(Int(*a as i32 + b)),
            (Int(a), SmallInt(b)) => Ok(Int(a + *b as i32)),
            (SmallInt(a), BigInt(b)) => Ok(BigInt(*a as i64 + b)),
            (BigInt(a), SmallInt(b)) => Ok(BigInt(a + *b as i64)),
            (Int(a), BigInt(b)) => Ok(BigInt(*a as i64 + b)),
            (BigInt(a), Int(b)) => Ok(BigInt(a + *b as i64)),

            // Integer to float promotion
            (SmallInt(a), Float(b)) => Ok(Float(*a as f32 + b)),
            (Float(a), SmallInt(b)) => Ok(Float(a + *b as f32)),
            (Int(a), Float(b)) => Ok(Float(*a as f32 + b)),
            (Float(a), Int(b)) => Ok(Float(a + *b as f32)),
            (BigInt(a), Float(b)) => Ok(Float(*a as f32 + b)),
            (Float(a), BigInt(b)) => Ok(Float(a + *b as f32)),

            (SmallInt(a), Double(b)) => Ok(Double(*a as f64 + b)),
            (Double(a), SmallInt(b)) => Ok(Double(a + *b as f64)),
            (Int(a), Double(b)) => Ok(Double(*a as f64 + b)),
            (Double(a), Int(b)) => Ok(Double(a + *b as f64)),
            (BigInt(a), Double(b)) => Ok(Double(*a as f64 + b)),
            (Double(a), BigInt(b)) => Ok(Double(a + *b as f64)),

            (Float(a), Double(b)) => Ok(Double(*a as f64 + b)),
            (Double(a), Float(b)) => Ok(Double(a + *b as f64)),

            // String concatenation
            (String(a), String(b)) => Ok(String(format!("{}{}", a, b))),
            (String(a), FixedString { data: b, .. }) => Ok(String(format!("{}{}", a, b))),
            (FixedString { data: a, .. }, String(b)) => Ok(String(format!("{}{}", a, b))),
            (FixedString { data: a, .. }, FixedString { data: b, .. }) => {
                Ok(String(format!("{}{}", a, b)))
            }
            _ => Err("Cannot perform addition on these value types".to_string()),
        }
    }

    /// Subtraction operation
    pub fn sub(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a.wrapping_sub(*b))),
            (Int(a), Int(b)) => Ok(Int(a.wrapping_sub(*b))),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a.wrapping_sub(*b))),
            (Float(a), Float(b)) => Ok(Float(a - b)),
            (Double(a), Double(b)) => Ok(Double(a - b)),

            // Cross-type operations
            (SmallInt(a), Int(b)) => Ok(Int(*a as i32 - b)),
            (Int(a), SmallInt(b)) => Ok(Int(a - *b as i32)),
            (SmallInt(a), BigInt(b)) => Ok(BigInt(*a as i64 - b)),
            (BigInt(a), SmallInt(b)) => Ok(BigInt(a - *b as i64)),
            (Int(a), BigInt(b)) => Ok(BigInt(*a as i64 - b)),
            (BigInt(a), Int(b)) => Ok(BigInt(a - *b as i64)),

            // Integer to float
            (SmallInt(a), Float(b)) => Ok(Float(*a as f32 - b)),
            (Float(a), SmallInt(b)) => Ok(Float(a - *b as f32)),
            (Int(a), Float(b)) => Ok(Float(*a as f32 - b)),
            (Float(a), Int(b)) => Ok(Float(a - *b as f32)),
            (BigInt(a), Float(b)) => Ok(Float(*a as f32 - b)),
            (Float(a), BigInt(b)) => Ok(Float(a - *b as f32)),

            (SmallInt(a), Double(b)) => Ok(Double(*a as f64 - b)),
            (Double(a), SmallInt(b)) => Ok(Double(a - *b as f64)),
            (Int(a), Double(b)) => Ok(Double(*a as f64 - b)),
            (Double(a), Int(b)) => Ok(Double(a - *b as f64)),
            (BigInt(a), Double(b)) => Ok(Double(*a as f64 - b)),
            (Double(a), BigInt(b)) => Ok(Double(a - *b as f64)),

            (Float(a), Double(b)) => Ok(Double(*a as f64 - b)),
            (Double(a), Float(b)) => Ok(Double(a - *b as f64)),

            _ => Err("Cannot perform subtraction on these value types".to_string()),
        }
    }

    /// Multiplication operation
    pub fn mul(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a.wrapping_mul(*b))),
            (Int(a), Int(b)) => Ok(Int(a.wrapping_mul(*b))),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a.wrapping_mul(*b))),
            (Float(a), Float(b)) => Ok(Float(a * b)),
            (Double(a), Double(b)) => Ok(Double(a * b)),

            // Cross-type operations
            (SmallInt(a), Int(b)) => Ok(Int(*a as i32 * b)),
            (Int(a), SmallInt(b)) => Ok(Int(a * *b as i32)),
            (SmallInt(a), BigInt(b)) => Ok(BigInt(*a as i64 * b)),
            (BigInt(a), SmallInt(b)) => Ok(BigInt(a * *b as i64)),
            (Int(a), BigInt(b)) => Ok(BigInt(*a as i64 * b)),
            (BigInt(a), Int(b)) => Ok(BigInt(a * *b as i64)),

            // Integer to float
            (SmallInt(a), Float(b)) => Ok(Float(*a as f32 * b)),
            (Float(a), SmallInt(b)) => Ok(Float(a * *b as f32)),
            (Int(a), Float(b)) => Ok(Float(*a as f32 * b)),
            (Float(a), Int(b)) => Ok(Float(a * *b as f32)),
            (BigInt(a), Float(b)) => Ok(Float(*a as f32 * b)),
            (Float(a), BigInt(b)) => Ok(Float(a * *b as f32)),

            (SmallInt(a), Double(b)) => Ok(Double(*a as f64 * b)),
            (Double(a), SmallInt(b)) => Ok(Double(a * *b as f64)),
            (Int(a), Double(b)) => Ok(Double(*a as f64 * b)),
            (Double(a), Int(b)) => Ok(Double(a * *b as f64)),
            (BigInt(a), Double(b)) => Ok(Double(*a as f64 * b)),
            (Double(a), BigInt(b)) => Ok(Double(a * *b as f64)),

            (Float(a), Double(b)) => Ok(Double(*a as f64 * b)),
            (Double(a), Float(b)) => Ok(Double(a * *b as f64)),

            _ => Err("Cannot perform multiplication on these value types".to_string()),
        }
    }

    /// Division operation
    pub fn div(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(SmallInt(a / b))
                }
            }
            (Int(a), Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Int(a / b))
                }
            }
            (BigInt(a), BigInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(BigInt(a / b))
                }
            }
            (Float(a), Float(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Float(a / b))
                }
            }
            (Double(a), Double(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(a / b))
                }
            }

            // Cross-type: promote to larger type
            (SmallInt(a), Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Int(*a as i32 / b))
                }
            }
            (Int(a), SmallInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Int(a / *b as i32))
                }
            }
            (Int(a), BigInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(BigInt(*a as i64 / b))
                }
            }
            (BigInt(a), Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(BigInt(a / *b as i64))
                }
            }

            // Integer to double for division
            (SmallInt(a), Double(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(*a as f64 / b))
                }
            }
            (Double(a), SmallInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(a / *b as f64))
                }
            }
            (Int(a), Double(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(*a as f64 / b))
                }
            }
            (Double(a), Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(a / *b as f64))
                }
            }
            (BigInt(a), Double(b)) => {
                if *b == 0.0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(*a as f64 / b))
                }
            }
            (Double(a), BigInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Double(a / *b as f64))
                }
            }

            _ => Err("Cannot perform division on these value types".to_string()),
        }
    }

    /// Modular operation
    pub fn rem(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(SmallInt(a % b))
                }
            }
            (Int(a), Int(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(Int(a % b))
                }
            }
            (BigInt(a), BigInt(b)) => {
                if *b == 0 {
                    Err("Division by zero".to_string())
                } else {
                    Ok(BigInt(a % b))
                }
            }
            _ => Err("Modulo operation is only supported for integer types".to_string()),
        }
    }

    /// Power operation
    pub fn pow(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => {
                if *b < 0 {
                    Err("Negative exponent not supported for integer power operation".to_string())
                } else {
                    Ok(SmallInt(a.pow(*b as u32)))
                }
            }
            (Int(a), Int(b)) => {
                if *b < 0 {
                    Err("Negative exponent not supported for integer power operation".to_string())
                } else {
                    Ok(Int(a.pow(*b as u32)))
                }
            }
            (BigInt(a), BigInt(b)) => {
                if *b < 0 {
                    Err("Negative exponent not supported for integer power operation".to_string())
                } else {
                    Ok(BigInt(a.pow(*b as u32)))
                }
            }
            (Float(a), Float(b)) => Ok(Float(a.powf(*b))),
            (Double(a), Double(b)) => Ok(Double(a.powf(*b))),
            (SmallInt(a), Double(b)) => Ok(Double((*a as f64).powf(*b))),
            (Double(a), SmallInt(b)) => Ok(Double(a.powi(*b as i32))),
            (Int(a), Double(b)) => Ok(Double((*a as f64).powf(*b))),
            (Double(a), Int(b)) => Ok(Double(a.powi(*b))),
            (BigInt(a), Double(b)) => Ok(Double((*a as f64).powf(*b))),
            (Double(a), BigInt(b)) => Ok(Double(a.powi(*b as i32))),
            _ => Err("Cannot perform power operation on these value types".to_string()),
        }
    }

    /// Negation operation
    pub fn neg(&self) -> Result<Value, String> {
        use Value::*;
        match self {
            SmallInt(a) => Ok(SmallInt(-a)),
            Int(a) => Ok(Int(-a)),
            BigInt(a) => Ok(BigInt(-a)),
            Float(a) => Ok(Float(-a)),
            Double(a) => Ok(Double(-a)),
            _ => Err("Negation is only supported for numeric types".to_string()),
        }
    }

    /// Logic and Operations
    pub fn and(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (Bool(a), Bool(b)) => Ok(Bool(*a && *b)),
            _ => Err("Logical AND is only supported for boolean types".to_string()),
        }
    }

    /// Logical OR operation
    pub fn or(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (Bool(a), Bool(b)) => Ok(Bool(*a || *b)),
            _ => Err("Logical OR is only supported for boolean types".to_string()),
        }
    }

    /// Logical NOT operation
    pub fn not(&self) -> Result<Value, String> {
        use Value::*;
        match self {
            Bool(a) => Ok(Bool(!a)),
            _ => Err("Logical NOT is only supported for boolean types".to_string()),
        }
    }

    /// Bitwise AND operation
    pub fn bit_and(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a & b)),
            (Int(a), Int(b)) => Ok(Int(a & b)),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a & b)),
            _ => Err("Bitwise AND is only supported for integer types".to_string()),
        }
    }

    /// Bitwise OR operation
    pub fn bit_or(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a | b)),
            (Int(a), Int(b)) => Ok(Int(a | b)),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a | b)),
            _ => Err("Bitwise OR is only supported for integer types".to_string()),
        }
    }

    /// Bitwise XOR operation
    pub fn bit_xor(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => Ok(SmallInt(a ^ b)),
            (Int(a), Int(b)) => Ok(Int(a ^ b)),
            (BigInt(a), BigInt(b)) => Ok(BigInt(a ^ b)),
            _ => Err("Bitwise XOR is only supported for integer types".to_string()),
        }
    }

    /// Left shift operation
    pub fn bit_shl(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => {
                if *b < 0 || *b >= 16 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(SmallInt(a << *b as u32))
                }
            }
            (Int(a), Int(b)) => {
                if *b < 0 || *b >= 32 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(Int(a << *b as u32))
                }
            }
            (BigInt(a), BigInt(b)) => {
                if *b < 0 || *b >= 64 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(BigInt(a << *b as u32))
                }
            }
            _ => Err("Bitwise left shift is only supported for integer types".to_string()),
        }
    }

    /// Right-shift operation
    pub fn bit_shr(&self, other: &Value) -> Result<Value, String> {
        use Value::*;
        match (self, other) {
            (SmallInt(a), SmallInt(b)) => {
                if *b < 0 || *b >= 16 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(SmallInt(a >> *b as u32))
                }
            }
            (Int(a), Int(b)) => {
                if *b < 0 || *b >= 32 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(Int(a >> *b as u32))
                }
            }
            (BigInt(a), BigInt(b)) => {
                if *b < 0 || *b >= 64 {
                    Err("Shift count out of range".to_string())
                } else {
                    Ok(BigInt(a >> *b as u32))
                }
            }
            _ => Err("Bitwise right shift is only supported for integer types".to_string()),
        }
    }

    /// Bitwise NOT operation
    pub fn bit_not(&self) -> Result<Value, String> {
        use Value::*;
        match self {
            SmallInt(a) => Ok(SmallInt(!a)),
            Int(a) => Ok(Int(!a)),
            BigInt(a) => Ok(BigInt(!a)),
            _ => Err("Bitwise NOT is only supported for integer types".to_string()),
        }
    }

    /// Absolute value operation
    pub fn abs(&self) -> Result<Value, String> {
        use Value::*;
        match self {
            SmallInt(a) => Ok(SmallInt(a.abs())),
            Int(a) => Ok(Int(a.abs())),
            BigInt(a) => Ok(BigInt(a.abs())),
            Float(a) => Ok(Float(a.abs())),
            Double(a) => Ok(Double(a.abs())),
            _ => Err("Absolute value is only supported for numeric types".to_string()),
        }
    }

    /// Get the length of a value (for strings, lists, maps, sets)
    pub fn len(&self) -> Result<Value, String> {
        use Value::*;
        match self {
            String(s) => Ok(Int(s.len() as i32)),
            FixedString { data, .. } => Ok(Int(data.len() as i32)),
            List(l) => Ok(Int(l.values.len() as i32)),
            Map(m) => Ok(Int(m.len() as i32)),
            Set(s) => Ok(Int(s.len() as i32)),
            Blob(b) => Ok(Int(b.len() as i32)),
            _ => Err(
                "Length operation is only supported for string, blob, list, map, or set types"
                    .to_string(),
            ),
        }
    }
}

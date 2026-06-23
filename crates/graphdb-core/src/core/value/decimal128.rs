//! Decimal128 type implementation
//!
//! This module implements the 128-bit decimal floating point type of the IEEE 754-2008 standard.
//!
//! ## Characteristics ##
//!
//! - 34-bit decimal precision
//! - Avoiding Precision Problems with Binary Floating Point Numbers
//! - Ideal for financial computing and scientific computing
//! - Compatible with MongoDB BSON Decimal128
//!
//! ## Usage scenarios
//!
//! - Financial applications (monetary calculations, interest rate calculations)
//! - Scientific computing (numerical calculations requiring high precision)
//! - Tax calculations (need to be precise to the nearest cent)
//! - Accounting systems (to avoid rounding errors)
//!
//! ## Performance considerations
//!
//! Decimal128 operations are slower than native floating-point numbers, but provide accurate decimal calculations.
//! For scenarios where high precision is not required, the Float type is recommended.

use dec::Decimal128;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Decimal128 Value Wrapper
///
/// Wraps the `dec::Decimal128` type to provide integration with the GraphDB type system.
///
/// ## Example
///
/// ```rust
/// use graphdb::core::value::decimal128::Decimal128Value;
///
/// let d1 = "123.456".parse::<Decimal128Value>().expect("parse failed");
/// let d2 = "789.012".parse::<Decimal128Value>().expect("parse failed");
/// let sum = &d1 + &d2;
/// assert_eq!(sum.to_string(), "912.468");
/// ```
#[derive(Debug, Clone)]
pub struct Decimal128Value {
    inner: Decimal128,
}

impl Serialize for Decimal128Value {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_string().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Decimal128Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Decimal128Value {
    /// Create a new Decimal128 value
    pub fn new(inner: Decimal128) -> Self {
        Self { inner }
    }

    /// Creating Decimal128 from i64
    pub fn from_i64(n: i64) -> Self {
        Self {
            inner: Decimal128::from(n),
        }
    }

    /// Creating Decimal128 from u64
    pub fn from_u64(n: u64) -> Self {
        Self {
            inner: Decimal128::from(n),
        }
    }

    /// Create Decimal128 from f64 (note: there may be a loss of precision)
    pub fn from_f64(n: f64) -> Option<Self> {
        let s = n.to_string();
        Self::from_str(&s).ok()
    }

    /// Get the internal Decimal128 value
    pub fn inner(&self) -> &Decimal128 {
        &self.inner
    }

    /// Get internal Decimal128 value (variable)
    pub fn inner_mut(&mut self) -> &mut Decimal128 {
        &mut self.inner
    }

    /// addition
    pub fn add(&self, other: &Self) -> Result<Self, String> {
        Ok(Self {
            inner: self.inner + other.inner,
        })
    }

    /// subtraction operation
    pub fn sub(&self, other: &Self) -> Result<Self, String> {
        Ok(Self {
            inner: self.inner - other.inner,
        })
    }

    /// multiplication
    pub fn mul(&self, other: &Self) -> Result<Self, String> {
        Ok(Self {
            inner: self.inner * other.inner,
        })
    }

    /// division
    pub fn div(&self, other: &Self) -> Result<Self, String> {
        if other.inner == Decimal128::ZERO {
            return Err("division error".to_string());
        }
        Ok(Self {
            inner: self.inner / other.inner,
        })
    }

    /// modulo operation (math.)
    pub fn rem(&self, other: &Self) -> Result<Self, String> {
        if other.inner == Decimal128::ZERO {
            return Err("division error".to_string());
        }
        Ok(Self {
            inner: self.inner % other.inner,
        })
    }

    /// absolute value
    pub fn abs(&self) -> Self {
        Self {
            inner: if self.inner < Decimal128::ZERO {
                -self.inner
            } else {
                self.inner
            },
        }
    }

    /// retrieve the opposite of what one intended
    pub fn neg(&self) -> Self {
        Self { inner: -self.inner }
    }

    /// Rounding to specified decimal places
    pub fn round_dp(&self, dp: u32) -> Self {
        let s = self.to_string();
        if let Some(dot_pos) = s.find('.') {
            let integer_part = &s[..dot_pos];
            let fractional_part = &s[dot_pos + 1..];
            if fractional_part.len() <= dp as usize {
                return self.clone();
            }
            let rounded_fractional = &fractional_part[..dp as usize];
            let rounded_str = format!("{}.{}", integer_part, rounded_fractional);
            Self::from_str(&rounded_str).unwrap_or_else(|_| self.clone())
        } else {
            self.clone()
        }
    }

    /// round down
    pub fn floor(&self) -> Self {
        let s = self.to_string();
        if let Some(dot_pos) = s.find('.') {
            let integer_part = &s[..dot_pos];
            Self::from_str(integer_part).unwrap_or_else(|_| self.clone())
        } else {
            self.clone()
        }
    }

    /// Round up
    pub fn ceil(&self) -> Self {
        let s = self.to_string();
        if let Some(dot_pos) = s.find('.') {
            let fractional_part = &s[dot_pos + 1..];
            if fractional_part.chars().all(|c| c == '0') {
                self.clone()
            } else {
                let integer_part = &s[..dot_pos];
                let int_value: i64 = integer_part.parse().unwrap_or(0);
                let ceil_value = if self.inner >= Decimal128::ZERO {
                    int_value + 1
                } else {
                    int_value
                };
                Self::from_i64(ceil_value)
            }
        } else {
            self.clone()
        }
    }

    /// Is it zero?
    pub fn is_zero(&self) -> bool {
        self.inner == Decimal128::ZERO
    }

    /// Whether the number is negative or not
    pub fn is_negative(&self) -> bool {
        self.inner < Decimal128::ZERO
    }

    /// Positive or not
    pub fn is_positive(&self) -> bool {
        self.inner > Decimal128::ZERO
    }

    /// Whether NaN
    pub fn is_nan(&self) -> bool {
        self.inner == Decimal128::NAN
    }
}

impl Default for Decimal128Value {
    fn default() -> Self {
        Self {
            inner: Decimal128::ZERO,
        }
    }
}

impl fmt::Display for Decimal128Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl FromStr for Decimal128Value {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Decimal128::from_str(s)
            .map(|inner| Self { inner })
            .map_err(|e| format!("Decimal128 parse failed: {}", e))
    }
}

impl std::ops::Add for &Decimal128Value {
    type Output = Decimal128Value;

    fn add(self, other: Self) -> Self::Output {
        Decimal128Value {
            inner: self.inner + other.inner,
        }
    }
}

impl std::ops::Sub for &Decimal128Value {
    type Output = Decimal128Value;

    fn sub(self, other: Self) -> Self::Output {
        Decimal128Value {
            inner: self.inner - other.inner,
        }
    }
}

impl std::ops::Mul for &Decimal128Value {
    type Output = Decimal128Value;

    fn mul(self, other: Self) -> Self::Output {
        Decimal128Value {
            inner: self.inner * other.inner,
        }
    }
}

impl std::ops::Div for &Decimal128Value {
    type Output = Decimal128Value;

    fn div(self, other: Self) -> Self::Output {
        Decimal128Value {
            inner: self.inner / other.inner,
        }
    }
}

impl std::ops::Rem for &Decimal128Value {
    type Output = Decimal128Value;

    fn rem(self, other: Self) -> Self::Output {
        Decimal128Value {
            inner: self.inner % other.inner,
        }
    }
}

impl std::ops::Neg for Decimal128Value {
    type Output = Decimal128Value;

    fn neg(self) -> Self::Output {
        Decimal128Value { inner: -self.inner }
    }
}

impl PartialEq for Decimal128Value {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for Decimal128Value {}

impl PartialOrd for Decimal128Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal128Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.inner < other.inner {
            std::cmp::Ordering::Less
        } else if self.inner > other.inner {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

impl std::hash::Hash for Decimal128Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.to_string().hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_str() {
        let d = "123.456".parse::<Decimal128Value>().expect("parse failed");
        assert_eq!(d.to_string(), "123.456");
    }

    #[test]
    fn test_from_i64() {
        let d = Decimal128Value::from_i64(123456);
        assert_eq!(d.to_string(), "123456");
    }

    #[test]
    fn test_add() {
        let d1 = "123.456".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "789.012".parse::<Decimal128Value>().expect("parse failed");
        let sum = &d1 + &d2;
        assert_eq!(sum.to_string(), "912.468");
    }

    #[test]
    fn test_sub() {
        let d1 = "789.012".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "123.456".parse::<Decimal128Value>().expect("parse failed");
        let diff = &d1 - &d2;
        assert_eq!(diff.to_string(), "665.556");
    }

    #[test]
    fn test_mul() {
        let d1 = "12.34".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "5.6".parse::<Decimal128Value>().expect("parse failed");
        let product = &d1 * &d2;
        assert_eq!(product.to_string(), "69.104");
    }

    #[test]
    fn test_div() {
        let d1 = "100.0".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "4.0".parse::<Decimal128Value>().expect("parse failed");
        let quotient = &d1 / &d2;
        assert_eq!(quotient.to_string(), "25");
    }

    #[test]
    fn test_div_by_zero() {
        let d1 = "100.0".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "0.0".parse::<Decimal128Value>().expect("parse failed");
        let result = d1.div(&d2);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "division error");
    }

    #[test]
    fn test_neg() {
        let d = "123.456".parse::<Decimal128Value>().expect("parse failed");
        let neg = -d.clone();
        assert_eq!(neg.to_string(), "-123.456");
    }

    #[test]
    fn test_abs() {
        let d1 = "-123.456".parse::<Decimal128Value>().expect("parse failed");
        let d2 = "123.456".parse::<Decimal128Value>().expect("parse failed");
        assert_eq!(d1.abs().to_string(), "123.456");
        assert_eq!(d2.abs().to_string(), "123.456");
    }

    #[test]
    fn test_round_dp() {
        let d = "123.456789"
            .parse::<Decimal128Value>()
            .expect("parse failed");
        let rounded = d.round_dp(2);
        assert_eq!(rounded.to_string(), "123.45");
    }

    #[test]
    fn test_floor() {
        let d = "123.789".parse::<Decimal128Value>().expect("parse failed");
        let floored = d.floor();
        assert_eq!(floored.to_string(), "123");
    }

    #[test]
    fn test_ceil() {
        let d = "123.789".parse::<Decimal128Value>().expect("parse failed");
        let ceiled = d.ceil();
        assert_eq!(ceiled.to_string(), "124");
    }

    #[test]
    fn test_is_zero() {
        let d = "0.0".parse::<Decimal128Value>().expect("parse failed");
        assert!(d.is_zero());
    }

    #[test]
    fn test_is_negative() {
        let d = "-123.456".parse::<Decimal128Value>().expect("parse failed");
        assert!(d.is_negative());
    }

    #[test]
    fn test_is_positive() {
        let d = "123.456".parse::<Decimal128Value>().expect("parse failed");
        assert!(d.is_positive());
    }

    #[test]
    fn test_finance_calculation() {
        let price = "19.99".parse::<Decimal128Value>().expect("parse failed");
        let quantity = "3".parse::<Decimal128Value>().expect("parse failed");
        let tax_rate = "0.08".parse::<Decimal128Value>().expect("parse failed");

        let subtotal = &price * &quantity;
        let tax = &subtotal * &tax_rate;
        let total = &subtotal + &tax;

        assert_eq!(subtotal.to_string(), "59.97");
        assert_eq!(tax.to_string(), "4.7976");
        assert_eq!(total.to_string(), "64.7676");
    }
}

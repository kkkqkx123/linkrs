//! Interval Type Module - PostgreSQL Compatible Time Interval
//!
//! This module provides Interval type support compatible with PostgreSQL.
//!
//! ## Features
//! - Calendar interval (years, months)
//! - Clock interval (days, hours, minutes, seconds, microseconds)
//! - Mixed interval support (e.g., "1 year 2 days 3 hours")
//! - ISO 8601 and PostgreSQL format parsing
//! - Arithmetic operations with Date/DateTime types
//!
//! ## PostgreSQL Compatibility
//! PostgreSQL stores intervals as:
//! - months: i32 (calendar months)
//! - days: i32 (clock days, separate from months for DST handling)
//! - microseconds: i64 (clock time, can exceed 24 hours)

use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;
use std::ops::{Add, Neg, Sub};

/// Interval Value Type - PostgreSQL Compatible
///
/// Represents a time span that can be:
/// - Calendar-based (years, months)
/// - Clock-based (days, hours, minutes, seconds)
/// - Mixed (both calendar and clock components)
///
/// # Examples
/// - "1 year 2 months" -> months: 14
/// - "3 days 4 hours" -> days: 3, microseconds: 4 * 3600 * 1_000_000
/// - "1 year 2 days" -> months: 12, days: 2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IntervalValue {
    /// Calendar months (can be negative)
    pub months: i32,
    /// Clock days (can be negative, separate from months for DST handling)
    pub days: i32,
    /// Clock microseconds (can be negative and exceed 24 hours)
    pub microseconds: i64,
}

impl IntervalValue {
    /// Create a new interval
    pub const fn new(months: i32, days: i32, microseconds: i64) -> Self {
        Self {
            months,
            days,
            microseconds,
        }
    }

    /// Create interval from years
    pub const fn from_years(years: i32) -> Self {
        Self {
            months: years * 12,
            days: 0,
            microseconds: 0,
        }
    }

    /// Create interval from months
    pub const fn from_months(months: i32) -> Self {
        Self {
            months,
            days: 0,
            microseconds: 0,
        }
    }

    /// Create interval from days
    pub const fn from_days(days: i32) -> Self {
        Self {
            months: 0,
            days,
            microseconds: 0,
        }
    }

    /// Create interval from hours
    pub const fn from_hours(hours: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            microseconds: hours * 3_600_000_000,
        }
    }

    /// Create interval from minutes
    pub const fn from_minutes(minutes: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            microseconds: minutes * 60_000_000,
        }
    }

    /// Create interval from seconds
    pub const fn from_seconds(seconds: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            microseconds: seconds * 1_000_000,
        }
    }

    /// Create interval from milliseconds
    pub const fn from_milliseconds(millis: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            microseconds: millis * 1_000,
        }
    }

    /// Create interval from microseconds
    pub const fn from_microseconds(micros: i64) -> Self {
        Self {
            months: 0,
            days: 0,
            microseconds: micros,
        }
    }

    /// Parse interval from PostgreSQL format string
    ///
    /// Supported formats:
    /// - "1 year 2 months 3 days 4 hours 5 minutes 6 seconds"
    /// - "1-2" (year-month)
    /// - "3 4:05:06" (day time)
    /// - "P1Y2M3DT4H5M6S" (ISO 8601)
    pub fn parse(s: &str) -> Result<Self, IntervalError> {
        let s = s.trim();

        // Try ISO 8601 format first
        if s.starts_with('P') {
            return Self::parse_iso8601(s);
        }

        // Try PostgreSQL format
        Self::parse_postgresql(s)
    }

    /// Parse ISO 8601 duration format: P[n]Y[n]M[n]DT[n]H[n]M[n]S
    fn parse_iso8601(s: &str) -> Result<Self, IntervalError> {
        if !s.starts_with('P') {
            return Err(IntervalError::InvalidFormat(s.to_string()));
        }

        let mut months = 0i32;
        let mut days = 0i32;
        let mut microseconds = 0i64;

        let chars = s[1..].chars().peekable();
        let mut num_str = String::new();
        let mut in_time_part = false;

        for c in chars {
            match c {
                'T' => {
                    in_time_part = true;
                    num_str.clear();
                }
                'Y' => {
                    let num: i32 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    months += num * 12;
                    num_str.clear();
                }
                'M' if !in_time_part => {
                    let num: i32 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    months += num;
                    num_str.clear();
                }
                'D' => {
                    let num: i32 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    days += num;
                    num_str.clear();
                }
                'H' => {
                    let num: i64 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    microseconds += num * 3_600_000_000;
                    num_str.clear();
                }
                'M' if in_time_part => {
                    let num: i64 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    microseconds += num * 60_000_000;
                    num_str.clear();
                }
                'S' => {
                    let num: f64 = num_str
                        .parse()
                        .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
                    microseconds += (num * 1_000_000.0) as i64;
                    num_str.clear();
                }
                c if c.is_ascii_digit() || c == '.' || c == '-' => {
                    num_str.push(c);
                }
                _ => return Err(IntervalError::InvalidFormat(s.to_string())),
            }
        }

        Ok(Self::new(months, days, microseconds))
    }

    /// Parse PostgreSQL interval format
    fn parse_postgresql(s: &str) -> Result<Self, IntervalError> {
        let mut months = 0i32;
        let mut days = 0i32;
        let mut microseconds = 0i64;

        // Try year-month format: "1-2" (1 year 2 months)
        if let Some(dash_pos) = s.find('-') {
            let before = s[..dash_pos].trim();
            let after = s[dash_pos + 1..].trim();
            if before.parse::<i32>().is_ok() && after.parse::<i32>().is_ok() {
                let years: i32 = before.parse().unwrap();
                let mons: i32 = after.parse().unwrap();
                return Ok(Self::new(years * 12 + mons, 0, 0));
            }
        }

        // Try day time format: "3 4:05:06" (3 days 4 hours 5 minutes 6 seconds)
        if let Some(space_pos) = s.find(' ') {
            let before = s[..space_pos].trim();
            let after = s[space_pos + 1..].trim();
            if let Ok(d) = before.parse::<i32>() {
                if after.contains(':') {
                    days = d;
                    microseconds = Self::parse_time(after)?;
                    return Ok(Self::new(0, days, microseconds));
                }
            }
        }

        // Parse word format: "1 year 2 months 3 days ..."
        let parts: Vec<&str> = s.split_whitespace().collect();
        let mut i = 0;

        while i < parts.len() {
            let num: f64 = parts[i]
                .parse()
                .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
            i += 1;

            if i >= parts.len() {
                return Err(IntervalError::InvalidFormat(s.to_string()));
            }

            let unit = parts[i].to_lowercase();
            i += 1;

            // Handle plural forms
            let unit = if unit.ends_with('s') && unit.len() > 1 {
                &unit[..unit.len() - 1]
            } else {
                &unit
            };

            match unit {
                "year" => months += (num * 12.0) as i32,
                "month" => months += num as i32,
                "day" => days += num as i32,
                "hour" => microseconds += (num * 3_600_000_000.0) as i64,
                "minute" => microseconds += (num * 60_000_000.0) as i64,
                "second" => microseconds += (num * 1_000_000.0) as i64,
                "millisecond" => microseconds += (num * 1_000.0) as i64,
                "microsecond" => microseconds += num as i64,
                "week" => days += (num * 7.0) as i32,
                "century" | "centurie" => months += (num * 1200.0) as i32,
                "decade" => months += (num * 120.0) as i32,
                "millennium" => months += (num * 12000.0) as i32,
                "quarter" => months += (num * 3.0) as i32,
                _ => return Err(IntervalError::InvalidUnit(unit.to_string())),
            }
        }

        Ok(Self::new(months, days, microseconds))
    }

    /// Parse time part (HH:MM:SS[.fraction])
    fn parse_time(s: &str) -> Result<i64, IntervalError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(IntervalError::InvalidFormat(s.to_string()));
        }

        let hours: i64 = parts[0]
            .parse()
            .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
        let minutes: i64 = parts[1]
            .parse()
            .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;

        // Handle optional fractional seconds
        let sec_parts: Vec<&str> = parts[2].split('.').collect();
        let seconds: i64 = sec_parts[0]
            .parse()
            .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;

        let mut micros = 0i64;
        if sec_parts.len() > 1 {
            let frac = format!("{:0<6}", sec_parts[1]);
            let frac_6: i64 = frac[..6.min(frac.len())]
                .parse()
                .map_err(|_| IntervalError::InvalidFormat(s.to_string()))?;
            micros = frac_6;
        }

        Ok(hours * 3_600_000_000 + minutes * 60_000_000 + seconds * 1_000_000 + micros)
    }

    /// Get total months (calendar component)
    pub const fn total_months(&self) -> i32 {
        self.months
    }

    /// Get total days (clock component, excluding months)
    pub const fn total_days(&self) -> i32 {
        self.days
    }

    /// Get total microseconds (clock component)
    pub const fn total_microseconds(&self) -> i64 {
        self.microseconds
    }

    /// Get years component
    pub const fn years(&self) -> i32 {
        self.months / 12
    }

    /// Get remaining months component (0-11)
    pub const fn months_component(&self) -> i32 {
        self.months % 12
    }

    /// Get hours component from microseconds
    pub const fn hours(&self) -> i64 {
        self.microseconds / 3_600_000_000
    }

    /// Get minutes component from microseconds
    pub const fn minutes(&self) -> i64 {
        (self.microseconds.abs() % 3_600_000_000) / 60_000_000
    }

    /// Get seconds component from microseconds
    pub const fn seconds(&self) -> i64 {
        (self.microseconds.abs() % 60_000_000) / 1_000_000
    }

    /// Get fractional seconds (microseconds component)
    pub const fn fractional_seconds(&self) -> i64 {
        self.microseconds.abs() % 1_000_000
    }

    /// Check if interval is positive
    pub fn is_positive(&self) -> bool {
        self.months > 0 || self.days > 0 || self.microseconds > 0
    }

    /// Check if interval is negative
    pub fn is_negative(&self) -> bool {
        self.months < 0 || self.days < 0 || self.microseconds < 0
    }

    /// Check if interval is zero
    pub fn is_zero(&self) -> bool {
        self.months == 0 && self.days == 0 && self.microseconds == 0
    }

    /// Normalize the interval (ensure consistent sign)
    pub fn normalize(&self) -> Self {
        // PostgreSQL doesn't fully normalize intervals
        // Each component keeps its own sign
        *self
    }

    /// Negate the interval
    pub const fn neg(&self) -> Self {
        Self::new(-self.months, -self.days, -self.microseconds)
    }

    /// Absolute value
    pub fn abs(&self) -> Self {
        Self::new(self.months.abs(), self.days.abs(), self.microseconds.abs())
    }

    /// Format as ISO 8601 duration
    pub fn to_iso8601(&self) -> String {
        let mut result = String::from("P");

        let years = self.years();
        let months = self.months_component();
        let days = self.days;

        if years != 0 {
            result.push_str(&format!("{}Y", years));
        }
        if months != 0 {
            result.push_str(&format!("{}M", months));
        }
        if days != 0 {
            result.push_str(&format!("{}D", days));
        }

        let hours = self.hours();
        let minutes = self.minutes();
        let seconds = self.seconds();
        let micros = self.fractional_seconds();

        if hours != 0 || minutes != 0 || seconds != 0 || micros != 0 {
            result.push('T');
            if hours != 0 {
                result.push_str(&format!("{}H", hours));
            }
            if minutes != 0 {
                result.push_str(&format!("{}M", minutes));
            }
            if seconds != 0 || micros != 0 {
                if micros != 0 {
                    result.push_str(&format!("{}.{:06}S", seconds, micros));
                } else {
                    result.push_str(&format!("{}S", seconds));
                }
            }
        }

        // If no components, return zero seconds
        if result == "P" {
            result.push_str("T0S");
        }

        result
    }

    /// Format as PostgreSQL style string
    pub fn to_postgresql(&self) -> String {
        let mut parts = Vec::new();

        let years = self.years();
        let months = self.months_component();
        let days = self.days;
        let hours = self.hours();
        let minutes = self.minutes();
        let seconds = self.seconds();
        let micros = self.fractional_seconds();

        if years != 0 {
            parts.push(format!(
                "{} year{}",
                years,
                if years == 1 { "" } else { "s" }
            ));
        }
        if months != 0 {
            parts.push(format!(
                "{} mon{}",
                months,
                if months == 1 { "" } else { "s" }
            ));
        }
        if days != 0 {
            parts.push(format!("{} day{}", days, if days == 1 { "" } else { "s" }));
        }

        if hours != 0 || minutes != 0 || seconds != 0 || micros != 0 {
            let time_str = if micros != 0 {
                format!("{:02}:{:02}:{:02}.{:06}", hours, minutes, seconds, micros)
            } else {
                format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
            };
            parts.push(time_str);
        }

        if parts.is_empty() {
            "00:00:00".to_string()
        } else {
            parts.join(" ")
        }
    }

    /// Estimate memory usage
    pub const fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    /// Zero interval
    pub const fn zero() -> Self {
        Self::new(0, 0, 0)
    }
}

impl Default for IntervalValue {
    fn default() -> Self {
        Self::zero()
    }
}

impl fmt::Display for IntervalValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_postgresql())
    }
}

impl Add for IntervalValue {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(
            self.months + other.months,
            self.days + other.days,
            self.microseconds + other.microseconds,
        )
    }
}

impl Sub for IntervalValue {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(
            self.months - other.months,
            self.days - other.days,
            self.microseconds - other.microseconds,
        )
    }
}

impl Neg for IntervalValue {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.months, -self.days, -self.microseconds)
    }
}

/// Interval Error Type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntervalError {
    InvalidFormat(String),
    InvalidUnit(String),
}

impl fmt::Display for IntervalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IntervalError::InvalidFormat(s) => write!(f, "Invalid interval format: {}", s),
            IntervalError::InvalidUnit(u) => write!(f, "Invalid interval unit: {}", u),
        }
    }
}

impl std::error::Error for IntervalError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interval_from_components() {
        let iv = IntervalValue::new(14, 3, 4_500_000_000);
        assert_eq!(iv.months, 14);
        assert_eq!(iv.days, 3);
        assert_eq!(iv.microseconds, 4_500_000_000);
    }

    #[test]
    fn test_interval_from_years() {
        let iv = IntervalValue::from_years(2);
        assert_eq!(iv.months, 24);
        assert_eq!(iv.days, 0);
        assert_eq!(iv.microseconds, 0);
    }

    #[test]
    fn test_interval_from_days() {
        let iv = IntervalValue::from_days(5);
        assert_eq!(iv.months, 0);
        assert_eq!(iv.days, 5);
        assert_eq!(iv.microseconds, 0);
    }

    #[test]
    fn test_interval_from_hours() {
        let iv = IntervalValue::from_hours(3);
        assert_eq!(iv.microseconds, 3 * 3_600_000_000);
    }

    #[test]
    fn test_interval_parse_iso8601() {
        let iv = IntervalValue::parse("P1Y2M3DT4H5M6S").unwrap();
        assert_eq!(iv.months, 14);
        assert_eq!(iv.days, 3);
        assert_eq!(
            iv.microseconds,
            4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000
        );
    }

    #[test]
    fn test_interval_parse_iso8601_fractional() {
        let iv = IntervalValue::parse("PT1.5S").unwrap();
        assert_eq!(iv.microseconds, 1_500_000);
    }

    #[test]
    fn test_interval_parse_postgresql() {
        let iv =
            IntervalValue::parse("1 year 2 months 3 days 4 hours 5 minutes 6 seconds").unwrap();
        assert_eq!(iv.months, 14);
        assert_eq!(iv.days, 3);
        assert_eq!(
            iv.microseconds,
            4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000
        );
    }

    #[test]
    fn test_interval_parse_year_month() {
        let iv = IntervalValue::parse("1-2").unwrap();
        assert_eq!(iv.months, 14);
    }

    #[test]
    fn test_interval_parse_day_time() {
        let iv = IntervalValue::parse("3 4:05:06").unwrap();
        assert_eq!(iv.days, 3);
        assert_eq!(
            iv.microseconds,
            4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000
        );
    }

    #[test]
    fn test_interval_parse_plural() {
        let iv = IntervalValue::parse("2 years 5 days").unwrap();
        assert_eq!(iv.months, 24);
        assert_eq!(iv.days, 5);
    }

    #[test]
    fn test_interval_add() {
        let iv1 = IntervalValue::from_days(3);
        let iv2 = IntervalValue::from_hours(12);
        let result = iv1 + iv2;
        assert_eq!(result.days, 3);
        assert_eq!(result.microseconds, 12 * 3_600_000_000);
    }

    #[test]
    fn test_interval_sub() {
        let iv1 = IntervalValue::from_days(5);
        let iv2 = IntervalValue::from_days(2);
        let result = iv1 - iv2;
        assert_eq!(result.days, 3);
    }

    #[test]
    fn test_interval_neg() {
        let iv = IntervalValue::new(1, 2, 3);
        let neg = iv.neg();
        assert_eq!(neg.months, -1);
        assert_eq!(neg.days, -2);
        assert_eq!(neg.microseconds, -3);
    }

    #[test]
    fn test_interval_to_iso8601() {
        let iv = IntervalValue::new(14, 3, 4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000);
        assert_eq!(iv.to_iso8601(), "P1Y2M3DT4H5M6S");
    }

    #[test]
    fn test_interval_to_postgresql() {
        let iv = IntervalValue::new(14, 3, 4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000);
        assert_eq!(iv.to_postgresql(), "1 year 2 mons 3 days 04:05:06");
    }

    #[test]
    fn test_interval_zero() {
        let iv = IntervalValue::zero();
        assert!(iv.is_zero());
        assert_eq!(iv.to_iso8601(), "PT0S");
    }

    #[test]
    fn test_interval_display() {
        let iv = IntervalValue::from_days(5);
        assert_eq!(format!("{}", iv), "5 days");
    }

    #[test]
    fn test_interval_years_months_components() {
        let iv = IntervalValue::new(26, 0, 0);
        assert_eq!(iv.years(), 2);
        assert_eq!(iv.months_component(), 2);
    }

    #[test]
    fn test_interval_time_components() {
        let iv = IntervalValue::from_hours(25)
            + IntervalValue::from_minutes(30)
            + IntervalValue::from_seconds(45);
        assert_eq!(iv.hours(), 25);
        assert_eq!(iv.minutes(), 30);
        assert_eq!(iv.seconds(), 45);
    }

    #[test]
    fn test_interval_abs() {
        let iv = IntervalValue::new(-1, -2, -3);
        let abs = iv.abs();
        assert_eq!(abs.months, 1);
        assert_eq!(abs.days, 2);
        assert_eq!(abs.microseconds, 3);
    }

    #[test]
    fn test_interval_parse_weeks() {
        let iv = IntervalValue::parse("2 weeks").unwrap();
        assert_eq!(iv.days, 14);
    }

    #[test]
    fn test_interval_parse_quarter() {
        let iv = IntervalValue::parse("1 quarter").unwrap();
        assert_eq!(iv.months, 3);
    }

    #[test]
    fn test_interval_parse_century() {
        let iv = IntervalValue::parse("1 century").unwrap();
        assert_eq!(iv.months, 1200);
    }

    #[test]
    fn test_interval_parse_millennium() {
        let iv = IntervalValue::parse("1 millennium").unwrap();
        assert_eq!(iv.months, 12000);
    }

    #[test]
    fn test_interval_parse_invalid() {
        let result = IntervalValue::parse("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_interval_parse_invalid_unit() {
        let result = IntervalValue::parse("1 foo");
        assert!(result.is_err());
    }

    #[test]
    fn test_interval_equality() {
        let iv1 = IntervalValue::from_days(3);
        let iv2 = IntervalValue::from_days(3);
        let iv3 = IntervalValue::from_days(4);
        assert_eq!(iv1, iv2);
        assert_ne!(iv1, iv3);
    }
}

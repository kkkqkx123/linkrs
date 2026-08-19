//! Date and Time Type Module
//!
//! This module defines types for dates, times, date-times, and intervals, as well as the related operations.

use crate::core::value::interval::IntervalValue;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// Simple date representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Hash)]
pub struct DateValue {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl DateValue {
    /// Add an interval
    pub fn add_interval(&mut self, interval: &IntervalValue) {
        if interval.months != 0 {
            self.add_months(interval.months);
        }

        if interval.days != 0 {
            self.add_days(interval.days as i64);
        }
    }

    /// Subtract the interval.
    pub fn sub_interval(&mut self, interval: &IntervalValue) {
        if interval.months != 0 {
            self.add_months(-interval.months);
        }

        if interval.days != 0 {
            self.add_days(-(interval.days as i64));
        }
    }

    fn add_months(&mut self, months: i32) {
        let mut new_month = self.month as i32 + months;
        let mut year_delta = 0;

        while new_month > 12 {
            new_month -= 12;
            year_delta += 1;
        }

        while new_month < 1 {
            new_month += 12;
            year_delta -= 1;
        }

        self.year += year_delta;
        self.month = new_month as u32;

        self.normalize_day();
    }

    fn add_days(&mut self, days: i64) {
        let total_days = Self::to_days(self) + days;
        *self = Self::from_days(total_days);
    }

    fn normalize_day(&mut self) {
        let days_in_month = Self::days_in_month(self.year, self.month);
        if self.day > days_in_month {
            self.day = days_in_month;
        }
    }

    fn days_in_month(year: i32, month: u32) -> u32 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if Self::is_leap_year(year) {
                    29
                } else {
                    28
                }
            }
            _ => panic!("Invalid month"),
        }
    }

    fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }

    pub fn to_days(&self) -> i64 {
        let year = self.year as i64;
        let month = self.month as i64;
        let day = self.day as i64;

        let a = (14 - month) / 12;
        let y = year + 4800 - a;
        let m = month + 12 * a - 3;

        day + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045
    }

    /// Convert a day count (days since 0000-03-01 in the proleptic Gregorian
    /// calendar, as produced by [`DateValue::to_days`]) back into a date.
    pub fn from_days(days: i64) -> Self {
        let a = days + 32044;
        let b = (4 * a + 3) / 146097;
        let c = a - (146097 * b) / 4;
        let d = (4 * c + 3) / 1461;
        let e = c - (1461 * d) / 4;
        let m = (5 * e + 2) / 153;

        let day = e - (153 * m + 2) / 5 + 1;
        let month = m + 3 - 12 * (m / 10);
        let year = 100 * b + d - 4800 + m / 10;

        DateValue {
            year: year as i32,
            month: month as u32,
            day: day as u32,
        }
    }
}

impl Default for DateValue {
    fn default() -> Self {
        DateValue {
            year: 1970,
            month: 1,
            day: 1,
        }
    }
}

impl DateValue {
    /// Estimate the memory usage of the date value
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl std::fmt::Display for DateValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Simple time representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Hash, Default)]
pub struct TimeValue {
    pub hour: u32,
    pub minute: u32,
    pub sec: u32,
    pub microsec: u32,
}

impl TimeValue {
    /// Add interval
    pub fn add_interval(&mut self, interval: &IntervalValue) {
        let total_microseconds = interval.microseconds;
        let mut new_microseconds = self.microsec as i64 + total_microseconds;

        while new_microseconds >= 86_400_000_000 {
            new_microseconds -= 86_400_000_000;
        }
        while new_microseconds < 0 {
            new_microseconds += 86_400_000_000;
        }

        self.microsec = (new_microseconds % 1_000_000) as u32;
        let total_seconds = new_microseconds / 1_000_000;

        let mut total_time =
            self.hour as i64 * 3600 + self.minute as i64 * 60 + self.sec as i64 + total_seconds;

        while total_time >= 86_400 {
            total_time -= 86_400;
        }
        while total_time < 0 {
            total_time += 86_400;
        }

        self.hour = (total_time / 3600) as u32;
        self.minute = ((total_time % 3600) / 60) as u32;
        self.sec = (total_time % 60) as u32;
    }

    /// Subtract interval
    pub fn sub_interval(&mut self, interval: &IntervalValue) {
        let neg_interval = interval.neg();
        self.add_interval(&neg_interval);
    }
}

impl TimeValue {
    /// Estimate the memory usage of the time value
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

impl std::fmt::Display for TimeValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.sec)
    }
}

/// Simple date and time representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Hash)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub sec: u32,
    pub microsec: u32,
}

impl DateTimeValue {
    /// Add interval
    pub fn add_interval(&mut self, interval: &IntervalValue) {
        let mut date = DateValue {
            year: self.year,
            month: self.month,
            day: self.day,
        };
        date.add_interval(interval);

        let mut time = TimeValue {
            hour: self.hour,
            minute: self.minute,
            sec: self.sec,
            microsec: self.microsec,
        };
        time.add_interval(interval);

        self.year = date.year;
        self.month = date.month;
        self.day = date.day;
        self.hour = time.hour;
        self.minute = time.minute;
        self.sec = time.sec;
        self.microsec = time.microsec;
    }

    /// Subtract interval
    pub fn sub_interval(&mut self, interval: &IntervalValue) {
        let mut date = DateValue {
            year: self.year,
            month: self.month,
            day: self.day,
        };
        date.sub_interval(interval);

        let mut time = TimeValue {
            hour: self.hour,
            minute: self.minute,
            sec: self.sec,
            microsec: self.microsec,
        };
        time.sub_interval(interval);

        self.year = date.year;
        self.month = date.month;
        self.day = date.day;
        self.hour = time.hour;
        self.minute = time.minute;
        self.sec = time.sec;
        self.microsec = time.microsec;
    }
}

impl Default for DateTimeValue {
    fn default() -> Self {
        DateTimeValue {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            sec: 0,
            microsec: 0,
        }
    }
}

impl DateTimeValue {
    /// Estimate the memory usage of the datetime value
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }

    /// Convert to micros since the Unix epoch (1970-01-01T00:00:00.000000),
    /// mirroring [`DateValue::to_days`] for the date part.
    ///
    /// For **normalized** fields the micros timeline is strictly monotonic in
    /// the field-wise order used by `value_compare::cmp_datetime`; the typed
    /// fast path relies on that (see `chunk/kind.rs` for the shared
    /// limitation on non-normalized fields).
    pub fn to_micros(&self) -> i64 {
        let date = DateValue {
            year: self.year,
            month: self.month,
            day: self.day,
        };
        // `DateValue::to_days` counts from JDN 0 (1970-01-01 is day
        // 2_440_588), so the epoch offset is subtracted to obtain
        // micros-since-epoch.
        (date.to_days() - 2_440_588) * 86_400_000_000
            + i64::from(self.hour) * 3_600_000_000
            + i64::from(self.minute) * 60_000_000
            + i64::from(self.sec) * 1_000_000
            + i64::from(self.microsec)
    }

    /// Convert micros since the Unix epoch back into a date-time.
    ///
    /// Pre-epoch values (negative micros) are handled with `div_euclid` /
    /// `rem_euclid`, so the result fields are always normalized.
    pub fn from_micros(micros: i64) -> Self {
        let days = micros.div_euclid(86_400_000_000) + 2_440_588;
        let time_of_day = micros.rem_euclid(86_400_000_000);
        let date = DateValue::from_days(days);
        DateTimeValue {
            year: date.year,
            month: date.month,
            day: date.day,
            hour: (time_of_day / 3_600_000_000) as u32,
            minute: ((time_of_day % 3_600_000_000) / 60_000_000) as u32,
            sec: ((time_of_day % 60_000_000) / 1_000_000) as u32,
            microsec: (time_of_day % 1_000_000) as u32,
        }
    }
}

impl std::fmt::Display for DateTimeValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.sec
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_micros_epoch_is_zero() {
        assert_eq!(DateTimeValue::default().to_micros(), 0);
    }

    #[test]
    fn test_to_micros_before_epoch_is_negative() {
        let dt = DateTimeValue {
            year: 1969,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            sec: 59,
            microsec: 999_999,
        };
        assert_eq!(dt.to_micros(), -1);
    }

    #[test]
    fn test_micros_roundtrip() {
        let dt = DateTimeValue {
            year: 2024,
            month: 2,
            day: 29,
            hour: 12,
            minute: 34,
            sec: 56,
            microsec: 789_012,
        };
        assert_eq!(DateTimeValue::from_micros(dt.to_micros()), dt);
        let pre_epoch = DateTimeValue {
            year: 1900,
            month: 6,
            day: 15,
            hour: 8,
            minute: 30,
            sec: 0,
            microsec: 1,
        };
        assert_eq!(DateTimeValue::from_micros(pre_epoch.to_micros()), pre_epoch);
    }

    #[test]
    fn test_micros_ordering_matches_field_order() {
        let a = DateTimeValue {
            year: 2023,
            month: 12,
            day: 31,
            hour: 23,
            minute: 59,
            sec: 59,
            microsec: 999_999,
        };
        let b = DateTimeValue {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            sec: 0,
            microsec: 0,
        };
        assert!(a.to_micros() < b.to_micros());
    }
}

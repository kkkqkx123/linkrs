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

    fn from_days(days: i64) -> Self {
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

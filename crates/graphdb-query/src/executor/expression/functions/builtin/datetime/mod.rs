mod arithmetic;
mod constructors;
mod epoch;
mod extraction;
mod formatting;
mod generate;
mod interval;
mod truncation;

use arithmetic::*;
use constructors::*;
use epoch::*;
use extraction::*;
use formatting::*;
use generate::*;
use interval::*;
use truncation::*;

use crate::executor::expression::ExpressionError;
use graphdb_core::Value;

#[cfg(test)]
mod tests;

define_function_enum! {
    pub enum DateTimeFunction {
        Now => {
            name: "now",
            arity: 0,
            variadic: false,
            description: "current timestamp",
            handler: execute_now
        },
        Date => {
            name: "date",
            arity: 1,
            variadic: false,
            description: "Date of creation",
            handler: execute_date
        },
        Time => {
            name: "time",
            arity: 1,
            variadic: false,
            description: "Creation time",
            handler: execute_time
        },
        DateTime => {
            name: "datetime",
            arity: 0,
            variadic: true,
            description: "Creation date and time",
            handler: execute_datetime
        },
        Year => {
            name: "year",
            arity: 1,
            variadic: false,
            description: "Year of extraction",
            handler: execute_year
        },
        Month => {
            name: "month",
            arity: 1,
            variadic: false,
            description: "Month of withdrawal",
            handler: execute_month
        },
        Day => {
            name: "day",
            arity: 1,
            variadic: false,
            description: "Withdrawal date",
            handler: execute_day
        },
        Hour => {
            name: "hour",
            arity: 1,
            variadic: false,
            description: "Withdrawal hours",
            handler: execute_hour
        },
        Minute => {
            name: "minute",
            arity: 1,
            variadic: false,
            description: "Extraction minutes",
            handler: execute_minute
        },
        Second => {
            name: "second",
            arity: 1,
            variadic: false,
            description: "withdrawal second",
            handler: execute_second
        },
        TimeStamp => {
            name: "timestamp",
            arity: 0,
            variadic: true,
            description: "Get current timestamp or convert datetime to timestamp",
            handler: execute_timestamp
        },
        DateAdd => {
            name: "date_add",
            arity: 2,
            variadic: false,
            description: "Add interval to date/datetime",
            handler: execute_date_add
        },
        DateSub => {
            name: "date_sub",
            arity: 2,
            variadic: false,
            description: "Subtract interval from date/datetime",
            handler: execute_date_sub
        },
        DateDiff => {
            name: "date_diff",
            arity: 2,
            variadic: false,
            description: "Calculate difference between two dates/datetimes",
            handler: execute_date_diff
        },
        DateTrunc => {
            name: "date_trunc",
            arity: 2,
            variadic: false,
            description: "Truncate date/datetime to specified precision",
            handler: execute_date_trunc
        },
        CurrentDate => {
            name: "current_date",
            arity: 0,
            variadic: false,
            description: "Get current date",
            handler: execute_current_date
        },
        CurrentTimestamp => {
            name: "current_timestamp",
            arity: 0,
            variadic: false,
            description: "Get current timestamp",
            handler: execute_current_timestamp
        },
        ToChar => {
            name: "to_char",
            arity: 2,
            variadic: false,
            description: "Format datetime as string",
            handler: execute_to_char
        },
        ToDate => {
            name: "to_date",
            arity: 1,
            variadic: false,
            description: "Convert string to date",
            handler: execute_to_date
        },
        Age => {
            name: "age",
            arity: 1,
            variadic: false,
            description: "Calculate age/interval from date/datetime to now",
            handler: execute_age
        },
        LastDay => {
            name: "last_day",
            arity: 1,
            variadic: false,
            description: "Get last day of the month",
            handler: execute_last_day
        },
        GenerateSeries => {
            name: "generate_series",
            arity: 2,
            variadic: true,
            description: "Generate a series of timestamps",
            handler: execute_generate_series
        },
        ToYears => {
            name: "to_years",
            arity: 1,
            variadic: false,
            description: "Convert number to years interval",
            handler: execute_to_years
        },
        ToMonths => {
            name: "to_months",
            arity: 1,
            variadic: false,
            description: "Convert number to months interval",
            handler: execute_to_months
        },
        ToDays => {
            name: "to_days",
            arity: 1,
            variadic: false,
            description: "Convert number to days interval",
            handler: execute_to_days
        },
        ToHours => {
            name: "to_hours",
            arity: 1,
            variadic: false,
            description: "Convert number to hours interval",
            handler: execute_to_hours
        },
        ToMinutes => {
            name: "to_minutes",
            arity: 1,
            variadic: false,
            description: "Convert number to minutes interval",
            handler: execute_to_minutes
        },
        ToSeconds => {
            name: "to_seconds",
            arity: 1,
            variadic: false,
            description: "Convert number to seconds interval",
            handler: execute_to_seconds
        },
        ToMilliseconds => {
            name: "to_milliseconds",
            arity: 1,
            variadic: false,
            description: "Convert number to milliseconds interval",
            handler: execute_to_milliseconds
        },
        ToMicroseconds => {
            name: "to_microseconds",
            arity: 1,
            variadic: false,
            description: "Convert number to microseconds interval",
            handler: execute_to_microseconds
        },
        Century => {
            name: "century",
            arity: 1,
            variadic: false,
            description: "Extract century from date/datetime",
            handler: execute_century
        },
        EpochMs => {
            name: "epoch_ms",
            arity: 1,
            variadic: false,
            description: "Convert date/datetime to epoch milliseconds",
            handler: execute_epoch_ms
        },
        ToTimestamp => {
            name: "to_timestamp",
            arity: 1,
            variadic: false,
            description: "Convert epoch seconds to timestamp",
            handler: execute_to_timestamp
        },
        ToEpochMs => {
            name: "to_epoch_ms",
            arity: 1,
            variadic: false,
            description: "Convert date/datetime to epoch milliseconds",
            handler: execute_to_epoch_ms
        },
        DatePart => {
            name: "date_part",
            arity: 2,
            variadic: false,
            description: "Extract date part (year, month, day, hour, minute, second, dow, doy, quarter)",
            handler: execute_date_part
        },
        DayName => {
            name: "day_name",
            arity: 1,
            variadic: false,
            description: "Get day name from date/datetime",
            handler: execute_day_name
        },
        MonthName => {
            name: "month_name",
            arity: 1,
            variadic: false,
            description: "Get month name from date/datetime",
            handler: execute_month_name
        },
    }
}

impl DateTimeFunction {
    pub fn execute_with_cache(
        &self,
        args: &[Value],
        _cache: &mut (),
    ) -> Result<Value, ExpressionError> {
        self.execute(args)
    }
}

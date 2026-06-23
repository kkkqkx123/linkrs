//! Query result formatting module
//!
//! This module provides formatters for converting query results into various output formats.
//! It is focused on presentation layer formatting for CLI display.
//!
//! # Available Formats
//! - `table`: ASCII table format (default)
//! - `vertical`: Vertical record display (like MySQL \G)
//! - `csv`: Comma-separated values
//! - `json`: Pretty-printed JSON
//! - `html`: HTML table format
//!
//! # Usage
//! ```rust
//! use graphdb_cli::output::{OutputFormat, OutputFormatter};
//! use graphdb_cli::client::QueryResult;
//!
//! let formatter = OutputFormatter::new()
//!     .with_format(OutputFormat::Json);
//!
//! let output = formatter.format_result(&query_result);
//! ```

pub mod csv;
pub mod formatter;
pub mod json;
pub mod pager;
pub mod table;

pub use formatter::{OutputFormat, OutputFormatter};

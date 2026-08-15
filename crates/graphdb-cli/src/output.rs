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
//! use graphdb_cli::client::QueryResult;
//! use graphdb_cli::output::{OutputFormat, OutputFormatter};
//!
//! let mut formatter = OutputFormatter::new();
//! formatter.set_format(OutputFormat::JSON);
//!
//! let result = QueryResult {
//!     columns: vec!["name".to_string()],
//!     rows: vec![std::collections::HashMap::from([(
//!         "name".to_string(),
//!         serde_json::json!("alice"),
//!     )])],
//!     row_count: 1,
//!     execution_time_ms: 0,
//!     rows_scanned: 1,
//!     error: None,
//! };
//!
//! let output = formatter.format_result(&result);
//! assert!(output.contains("alice"), "JSON output must contain the row value");
//! ```

pub mod csv;
pub mod formatter;
pub mod json;
pub mod pager;
pub mod table;

pub use formatter::{OutputFormat, OutputFormatter};

//! General output infrastructure module
//!
//! This module provides low-level output primitives for the CLI.
//! It handles output destinations (stdout, stderr, files), formatting utilities,
//! and stream management.
//!
//! # Modules
//! - `manager`: Centralized output control with format selection
//! - `writer`: Various writer implementations (stdout, stderr, file, multi)
//! - `stream`: Stream output for file and console redirection
//! - `config`: Output configuration and modes
//! - `json`: JSON serialization utilities
//! - `table`: Table formatting utilities
//!
//! # Usage
//! ```rust
//! use graphdb_cli::utils::output;
//!
//! // Simple output
//! output::println("Hello, World!").unwrap();
//! output::print_success("Operation completed").unwrap();
//!
//! // With custom manager
//! use graphdb_cli::utils::output::{OutputManager, Format};
//! let manager = OutputManager::new()
//!     .with_format(Format::Json);
//! manager.println("{ \"status\": \"ok\" }").unwrap();
//! ```

// Error types
mod error;
pub use error::{OutputError, Result};

// Writer implementations
mod writer;
pub use writer::{FileWriter, MultiWriter, StderrWriter, StdoutWriter};

// Manager and format
mod manager;
pub use manager::{
    get_default_manager, get_global_format, print, print_error, print_info, print_success,
    print_warning, println, set_global_format, Format, OutputManager,
};

// JSON formatter
mod json;
pub use json::{
    print_json, print_json_compact, print_json_compact_to, print_json_to, to_json_string,
    to_json_string_compact, JsonFormatter,
};

// Table formatter
mod table;
pub use table::{print_table, print_table_to, TableFormatter};

// Configuration
mod config;
pub use config::{OutputConfig, OutputMode};

// Stream output
mod stream;
pub use stream::StreamOutput;

/// Initialize the output module with default settings
pub fn init() {
    // The global manager is lazily initialized on first use
}

/// Initialize the output module with a specific format
pub fn init_with_format(format: Format) {
    set_global_format(format);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Ensure all public types are accessible
        let _ = Format::Plain;
        let _ = Format::Json;
        let _ = Format::Table;
    }

    #[test]
    fn test_output_manager_creation() {
        let manager = OutputManager::new();
        assert_eq!(manager.format(), Format::Plain);
    }
}

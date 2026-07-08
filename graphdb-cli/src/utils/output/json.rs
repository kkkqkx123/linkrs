//! JSON formatter for output module

use std::io::Write;

use serde::Serialize;

use super::Result;

/// JSON formatter with configurable options
pub struct JsonFormatter {
    indent: String,
    prefix: String,
    escape_html: bool,
}

impl JsonFormatter {
    /// Create a new JSON formatter with default settings (pretty print)
    pub fn new() -> Self {
        Self {
            indent: "  ".to_string(),
            prefix: String::new(),
            escape_html: true,
        }
    }

    /// Create a new JSON formatter for compact output
    pub fn compact() -> Self {
        Self {
            indent: String::new(),
            prefix: String::new(),
            escape_html: true,
        }
    }

    /// Set the indent string
    pub fn with_indent(mut self, indent: &str) -> Self {
        self.indent = indent.to_string();
        self
    }

    /// Set the prefix string
    pub fn with_prefix(mut self, prefix: &str) -> Self {
        self.prefix = prefix.to_string();
        self
    }

    /// Set whether to escape HTML characters
    pub fn with_escape_html(mut self, escape: bool) -> Self {
        self.escape_html = escape;
        self
    }

    /// Format data as JSON and write to writer
    pub fn format<W: Write, T: Serialize>(&self, data: &T, writer: &mut W) -> Result<()> {
        if self.indent.is_empty() {
            // Compact format
            let json = serde_json::to_string(data)?;
            writeln!(writer, "{}", json)?;
        } else {
            // Pretty format
            let json = serde_json::to_string_pretty(data)?;
            writeln!(writer, "{}", json)?;
        }
        writer.flush()?;
        Ok(())
    }

    /// Format data as JSON string
    pub fn format_to_string<T: Serialize>(&self, data: &T) -> Result<String> {
        if self.indent.is_empty() {
            Ok(serde_json::to_string(data)?)
        } else {
            Ok(serde_json::to_string_pretty(data)?)
        }
    }
}

impl Default for JsonFormatter {
    fn default() -> Self {
        Self::new()
    }
}

/// Print data as JSON to the specified writer
pub fn print_json_to<W: Write, T: Serialize>(writer: &mut W, data: &T) -> Result<()> {
    JsonFormatter::new().format(data, writer)
}

/// Print data as compact JSON to the specified writer
pub fn print_json_compact_to<W: Write, T: Serialize>(writer: &mut W, data: &T) -> Result<()> {
    JsonFormatter::compact().format(data, writer)
}

/// Print data as JSON using global stdout
pub fn print_json<T: Serialize>(data: &T) -> Result<()> {
    use super::manager::get_default_manager;

    let manager = get_default_manager();
    let mut stdout = manager.stdout();
    print_json_to(&mut *stdout, data)
}

/// Print data as compact JSON using global stdout
pub fn print_json_compact<T: Serialize>(data: &T) -> Result<()> {
    use super::manager::get_default_manager;

    let manager = get_default_manager();
    let mut stdout = manager.stdout();
    print_json_compact_to(&mut *stdout, data)
}

/// Convert data to JSON string (pretty format)
pub fn to_json_string<T: Serialize>(data: &T) -> Result<String> {
    JsonFormatter::new().format_to_string(data)
}

/// Convert data to compact JSON string
pub fn to_json_string_compact<T: Serialize>(data: &T) -> Result<String> {
    JsonFormatter::compact().format_to_string(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[derive(Serialize)]
    struct TestData {
        name: String,
        value: i32,
    }

    #[test]
    fn test_json_formatter_pretty() {
        let formatter = JsonFormatter::new();
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let json = formatter.format_to_string(&data).unwrap();
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"test\""));
        assert!(json.contains("\"value\""));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_json_formatter_compact() {
        let formatter = JsonFormatter::compact();
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let json = formatter.format_to_string(&data).unwrap();
        assert!(json.contains("\"name\""));
        assert!(!json.contains("\n")); // Compact should not have newlines
    }

    #[test]
    fn test_print_json_to() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let mut cursor = Cursor::new(Vec::new());
        print_json_to(&mut cursor, &data).unwrap();

        let output = String::from_utf8(cursor.into_inner()).unwrap();
        assert!(output.contains("\"name\""));
        assert!(output.contains("\"test\""));
    }

    #[test]
    fn test_to_json_string() {
        let data = TestData {
            name: "test".to_string(),
            value: 42,
        };

        let json = to_json_string(&data).unwrap();
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"value\""));
    }
}

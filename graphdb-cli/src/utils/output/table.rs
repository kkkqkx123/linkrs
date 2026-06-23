//! Table formatter for output module

use std::io::Write;

use super::Result;

/// Table formatter for structured tabular output
pub struct TableFormatter {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    column_widths: Vec<usize>,
    max_column_width: usize,
}

impl TableFormatter {
    /// Create a new table formatter
    pub fn new() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            column_widths: Vec::new(),
            max_column_width: 50,
        }
    }

    /// Set the maximum column width (for truncation)
    pub fn with_max_column_width(mut self, width: usize) -> Self {
        self.max_column_width = width;
        self
    }

    /// Set the table headers
    pub fn set_headers(&mut self, headers: &[&str]) {
        self.headers = headers.iter().map(|h| h.to_string()).collect();
        self.calculate_column_widths();
    }

    /// Add a row to the table
    pub fn add_row(&mut self, row: &[&str]) {
        let row: Vec<String> = row.iter().map(|c| c.to_string()).collect();
        self.rows.push(row);
        self.calculate_column_widths();
    }

    /// Add a row from string vector
    pub fn add_row_strings(&mut self, row: Vec<String>) {
        self.rows.push(row);
        self.calculate_column_widths();
    }

    /// Calculate column widths based on content
    fn calculate_column_widths(&mut self) {
        let num_columns = self.headers.len();
        if num_columns == 0 {
            return;
        }

        let mut widths: Vec<usize> = self.headers.iter().map(|h| h.len()).collect();

        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < widths.len() {
                    widths[i] = widths[i].max(cell.len());
                }
            }
        }

        // Cap at max_column_width
        self.column_widths = widths
            .into_iter()
            .map(|w| w.min(self.max_column_width))
            .collect();
    }

    /// Format a cell with truncation or padding
    fn format_cell(&self, content: &str, width: usize) -> String {
        if content.len() > width {
            if width > 3 {
                format!("{}...", &content[..width - 3])
            } else {
                content[..width].to_string()
            }
        } else {
            format!("{:width$}", content, width = width)
        }
    }

    /// Render the table to a writer
    pub fn render<W: Write>(&self, writer: &mut W) -> Result<()> {
        if self.headers.is_empty() {
            return Ok(());
        }

        let total_width =
            self.column_widths.iter().sum::<usize>() + self.column_widths.len() * 3 + 1;

        // Top border
        writeln!(writer, "{}", "-".repeat(total_width))?;

        // Headers
        write!(writer, "| ")?;
        for (i, header) in self.headers.iter().enumerate() {
            let width = self.column_widths.get(i).copied().unwrap_or(10);
            write!(writer, "{} | ", self.format_cell(header, width))?;
        }
        writeln!(writer)?;

        // Header separator
        writeln!(writer, "{}", "-".repeat(total_width))?;

        // Rows
        for row in &self.rows {
            write!(writer, "| ")?;
            for (i, cell) in row.iter().enumerate() {
                let width = self.column_widths.get(i).copied().unwrap_or(10);
                write!(writer, "{} | ", self.format_cell(cell, width))?;
            }
            writeln!(writer)?;
        }

        // Bottom border
        writeln!(writer, "{}", "-".repeat(total_width))?;

        writer.flush()?;
        Ok(())
    }

    /// Render to string
    pub fn render_to_string(&self) -> Result<String> {
        let mut buf = Vec::new();
        self.render(&mut buf)?;
        Ok(String::from_utf8_lossy(&buf).to_string())
    }
}

impl Default for TableFormatter {
    fn default() -> Self {
        Self::new()
    }
}

/// Print a table to the specified writer
pub fn print_table_to<W: Write>(
    writer: &mut W,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<()> {
    let mut formatter = TableFormatter::new();
    formatter.set_headers(headers);
    for row in rows {
        formatter.add_row_strings(row.clone());
    }
    formatter.render(writer)
}

/// Print a table using global stdout
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) -> Result<()> {
    use super::manager::get_default_manager;

    let manager = get_default_manager();
    let mut stdout = manager.stdout();
    print_table_to(&mut *stdout, headers, rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_table_formatter() {
        let mut formatter = TableFormatter::new();
        formatter.set_headers(&["Name", "Age", "City"]);
        formatter.add_row(&["Alice", "30", "New York"]);
        formatter.add_row(&["Bob", "25", "Los Angeles"]);

        let output = formatter.render_to_string().unwrap();
        assert!(output.contains("Name"));
        assert!(output.contains("Alice"));
        assert!(output.contains("Bob"));
        assert!(output.contains("-")); // Border lines
    }

    #[test]
    fn test_table_truncation() {
        let mut formatter = TableFormatter::new().with_max_column_width(5);
        formatter.set_headers(&["VeryLongHeader"]);
        formatter.add_row(&["VeryLongContent"]);

        let output = formatter.render_to_string().unwrap();
        assert!(output.contains("Ve...")); // Truncated content
    }

    #[test]
    fn test_print_table_to() {
        let headers = ["Name", "Value"];
        let rows = vec![
            vec!["Item1".to_string(), "100".to_string()],
            vec!["Item2".to_string(), "200".to_string()],
        ];

        let mut cursor = Cursor::new(Vec::new());
        print_table_to(&mut cursor, &headers, &rows).unwrap();

        let output = String::from_utf8(cursor.into_inner()).unwrap();
        assert!(output.contains("Name"));
        assert!(output.contains("Item1"));
        assert!(output.contains("Item2"));
    }

    #[test]
    fn test_empty_table() {
        let formatter = TableFormatter::new();
        let output = formatter.render_to_string().unwrap();
        assert!(output.is_empty());
    }
}
